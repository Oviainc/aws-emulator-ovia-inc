// SPDX-License-Identifier: CC0-1.0

//! The DATA a dataset import job imports: reading the CSV files its `DataSource` names
//! out of the emulator's own object storage, and turning them into the rows a dataset
//! holds.
//!
//! # What is real here
//!
//! Amazon Personalize copies the objects at the `dataLocation` into the dataset and
//! trains on what it read. This module does the same thing against the emulator's own
//! Amazon S3, in process through the framework's callback — the same transport a
//! client's own S3 request takes — so an import reads exactly the bytes that were
//! uploaded and a dataset holds no record nothing put there.
//!
//! A read that cannot be carried out is an ERROR, never an empty success: the job that
//! asked for it ends in the modelled `CREATE FAILED` status carrying the reason, and
//! nothing is imported.

use std::collections::{BTreeMap, HashMap};

/// One imported record: its CSV columns, keyed by the column name the file's header row
/// spells them with.
///
/// A `BTreeMap`, so the columns of a row are walked in a stable order whatever order the
/// header listed them in.
pub type Row = BTreeMap<String, String>;

/// The most objects one import job reads, and the most bytes it reads from one object
/// and in total.
///
/// Amazon Personalize publishes its own per-dataset quotas; these are the EMULATOR's
/// bounds on the work one request does, so no request can make this process read an
/// unbounded amount of object storage. An import that points past them is not silently
/// truncated: the job ends in the modelled `CREATE FAILED` state naming the bound it hit.
pub const MAX_IMPORT_OBJECTS: usize = 50;
/// The most bytes read from one import object (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_IMPORT_OBJECT_BYTES: usize = 4 * 1024 * 1024;
/// The most bytes read across one import (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_IMPORT_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// The most rows one import stores (see [`MAX_IMPORT_OBJECTS`]).
pub const MAX_IMPORT_ROWS: usize = 20_000;
/// The most rows one dataset retains across every import into it (see
/// [`MAX_IMPORT_OBJECTS`]). An `INCREMENTAL` import "append[s] the new records to the
/// existing data", so without this an unbounded number of appends would grow one
/// dataset without limit.
///
/// Checked BEFORE a single row is applied ([`apply_import`]), so the work one submission
/// costs is bounded before it is done rather than after.
pub const MAX_DATASET_ROWS: usize = 20_000;

/// The URI scheme Amazon Personalize imports from in this emulator.
///
/// `DataSource$dataLocation`'s modelled pattern admits `s3`, `http` and `https`, and
/// only the first names storage this emulator serves, so a job pointed anywhere else is
/// refused at the submission rather than recorded as a run that imported nothing.
pub const IMPORT_SCHEME: &str = "s3://";

/// An `s3://bucket/key` path split into its parts.
#[derive(Debug, Clone)]
pub struct S3Location {
    /// The bucket the import files live in.
    pub bucket: String,
    /// The key, or the key prefix when the path names a folder — "If your CSV files are
    /// in a folder in your Amazon S3 bucket and you want your import job [...] to
    /// consider multiple files, you can specify the path to the folder"
    /// (`DataSource$dataLocation` doc).
    pub key: String,
}

/// Split a `dataLocation` into bucket and key prefix; `None` for a value that names no
/// Amazon S3 bucket.
pub fn parse_data_location(path: &str) -> Option<S3Location> {
    let rest = path.strip_prefix(IMPORT_SCHEME)?;
    let (bucket, key) = match rest.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| S3Location {
        bucket: bucket.to_string(),
        key: key.to_string(),
    })
}

/// The data location a `DataSource` names, when it names one at all.
///
/// `dataLocation` is an OPTIONAL member of a required structure, so a request may carry
/// a data source that names nothing; such a job has nothing to read and is refused where
/// it is submitted.
pub fn present_data_location(source: &crate::ops::types::DataSource) -> Option<String> {
    source
        .data_location
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated
/// service: it routes in process, so no network client is configured here. `region` is
/// the request's own signing region, so an import reads where the caller's own S3
/// requests go.
pub fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so every
        // request addresses its bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Read every object under `location` as UTF-8 text, paired with the path it was read
/// from, bounded by [`MAX_IMPORT_OBJECTS`] and the byte bounds beside it.
///
/// `Err` is the reason the import could not be carried out, ready for the job's failure
/// state.
pub async fn read_objects(
    client: &::aws_sdk_s3::Client,
    location: &S3Location,
    path: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut keys: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(&location.bucket)
            .prefix(&location.key)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "Could not list the data at {path}: {}",
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending in the delimiter is a folder marker, not an import file.
            if key.ends_with('/') {
                continue;
            }
            // The bound is checked BEFORE the entry is buffered, so not even one object
            // past it is held.
            if keys.len() >= MAX_IMPORT_OBJECTS {
                return Err(format!(
                    "The data at {path} is held in more than the {MAX_IMPORT_OBJECTS} files \
                     this emulator imports in one job."
                ));
            }
            keys.push((
                key.to_string(),
                usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(usize::MAX),
            ));
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    if keys.is_empty() {
        return Err(format!("No data was found at {path}."));
    }
    // The bounds are decided from the LISTED sizes, before a single object is fetched,
    // so an import past them costs no buffering at all.
    if let Some((key, size)) = keys.iter().find(|(_, size)| *size > MAX_IMPORT_OBJECT_BYTES) {
        return Err(format!(
            "The file s3://{}/{key} is {size} bytes, more than the {MAX_IMPORT_OBJECT_BYTES} \
             bytes this emulator imports from one file.",
            location.bucket,
        ));
    }
    let listed: usize = keys.iter().map(|(_, size)| *size).sum();
    if listed > MAX_IMPORT_TOTAL_BYTES {
        return Err(format!(
            "The data at {path} is {listed} bytes, more than the {MAX_IMPORT_TOTAL_BYTES} bytes \
             this emulator imports in one job."
        ));
    }

    let mut objects = Vec::with_capacity(keys.len());
    let mut remaining = MAX_IMPORT_TOTAL_BYTES;
    for (key, _) in keys {
        let object = client
            .get_object()
            .bucket(&location.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|error| {
                format!(
                    "Could not read s3://{}/{key}: {}",
                    location.bucket,
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error),
                )
            })?;
        let body = object
            .body
            .collect()
            .await
            .map_err(|error| format!("Could not read s3://{}/{key}: {error}", location.bucket))?
            .into_bytes();
        if body.len() > remaining {
            return Err(format!(
                "The data at {path} is larger than the {MAX_IMPORT_TOTAL_BYTES} bytes this \
                 emulator imports in one job."
            ));
        }
        remaining -= body.len();
        let text = String::from_utf8(body.to_vec()).map_err(|_| {
            format!(
                "s3://{}/{key} is not UTF-8 text; Amazon Personalize imports CSV files.",
                location.bucket,
            )
        })?;
        objects.push((format!("s3://{}/{key}", location.bucket), text));
    }
    Ok(objects)
}

/// The rows `objects` hold, read as the comma-delimited CSV Amazon Personalize imports
/// ("The data must be in CSV format", with a header row naming the schema's fields).
///
/// Every row is keyed by the header row's own column names, so which column a value
/// belongs to comes from the file rather than from a position this emulator assumed. A
/// file whose rows do not match its header, or that carries no header at all, is an
/// error: the job that read it failed, and a row this reader could not place would
/// otherwise be imported under the wrong column.
pub fn read_rows(objects: &[(String, String)]) -> Result<Vec<Row>, String> {
    let mut rows: Vec<Row> = Vec::new();
    for (path, body) in objects {
        if body.trim().is_empty() {
            continue;
        }
        let mut reader = ::csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(false)
            .trim(::csv::Trim::All)
            .from_reader(body.as_bytes());
        let headers: Vec<String> = reader
            .headers()
            .map_err(|error| format!("The header row of {path} could not be read: {error}"))?
            .iter()
            .map(|column| column.to_string())
            .collect();
        if headers.iter().all(|column| column.is_empty()) {
            return Err(format!(
                "{path} carries no header row naming the columns of its records."
            ));
        }
        for (index, record) in reader.records().enumerate() {
            let record = record
                .map_err(|error| format!("Row {} of {path} could not be read: {error}", index + 1))?;
            if record.iter().all(|field| field.is_empty()) {
                continue;
            }
            if rows.len() >= MAX_IMPORT_ROWS {
                return Err(format!(
                    "The data being imported holds more than the {MAX_IMPORT_ROWS} records this \
                     emulator imports in one job."
                ));
            }
            rows.push(
                headers
                    .iter()
                    .cloned()
                    .zip(record.iter().map(str::to_string))
                    .filter(|(_, value)| !value.is_empty())
                    .collect(),
            );
        }
    }
    Ok(rows)
}

/// Read the data at `path` and return the rows it holds, or the reason the import could
/// not be carried out.
///
/// Held outside a handler's store lock: the S3 calls are awaited with no lock held.
pub async fn run_import(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    path: &str,
) -> Result<Vec<Row>, String> {
    let location = parse_data_location(path)
        .ok_or_else(|| format!("The data source {path} names no Amazon S3 bucket."))?;
    let client = s3_client(ctx, region);
    let objects = read_objects(&client, &location, path).await?;
    let rows = read_rows(&objects)?;
    if rows.is_empty() {
        return Err(format!("The data at {path} holds no records to import."));
    }
    Ok(rows)
}

/// The value of the column `name` in `row`, matched the way Amazon Personalize matches a
/// schema field name — case-insensitively, exactly as `CreateSchema` checks the fields a
/// document declares — or `None` when the row carries no such column.
pub fn column<'a>(row: &'a Row, name: &str) -> Option<&'a str> {
    row.iter()
        .find(|(column, _)| column.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

/// The column of an Interactions or Items row that names the ITEM, matched
/// case-insensitively.
///
/// Source: the fields Amazon Personalize requires of each dataset type's schema, which
/// this crate states once in [`crate::state::REQUIRED_SCHEMA_FIELDS`] — an interactions
/// record is keyed by user, item and time, an items record by `item_id`.
pub const ITEM_ID_COLUMN: &str = "item_id";
/// The column that names the USER, from the same source.
pub const USER_ID_COLUMN: &str = "user_id";
/// The column of an Actions or Action-interactions row that names the ACTION, from the
/// same source.
pub const ACTION_ID_COLUMN: &str = "action_id";

/// The column whose value IDENTIFIES a record of `dataset`, or `None` for a dataset
/// whose records are events rather than entities.
///
/// An Items, Users or Actions dataset holds one record per entity, keyed by that
/// entity's id — which is why an `INCREMENTAL` import of one "replaces any record with
/// the same ID with the new one" (`CreateDatasetImportJob$importMode` doc). An
/// interactions dataset holds a STREAM, where two rows with the same ids are two
/// interactions, so it has no such key.
pub fn record_id_column(dataset: &crate::state::DatasetRecord) -> Option<&'static str> {
    use crate::state::is_dataset_type;

    if is_dataset_type(dataset, "Items") {
        Some(ITEM_ID_COLUMN)
    } else if is_dataset_type(dataset, "Users") {
        Some(USER_ID_COLUMN)
    } else if is_dataset_type(dataset, "Actions") {
        Some(ACTION_ID_COLUMN)
    } else {
        None
    }
}

/// Import `rows` into `dataset` under `import_mode`, and report how many records the
/// job imported.
///
/// "By default, a dataset import job replaces any existing data in the dataset that you
/// imported in bulk. To add new records without replacing existing data, specify
/// INCREMENTAL [...] Amazon Personalize replaces any record with the same ID with the
/// new one" (`CreateDatasetImportJob` / `importMode` docs). So a `FULL` import is the
/// dataset's new bulk content, and an `INCREMENTAL` one adds to it, replacing an entity
/// a row names again.
///
/// The work is LINEAR in the rows it touches and bounded before any of it is done: the
/// dataset's ceiling ([`MAX_DATASET_ROWS`]) is checked against the retained and incoming
/// counts first, and the entity ids already stored are indexed in ONE pass, so a row
/// that replaces an entity is resolved by lookup rather than by scanning what is stored.
///
/// `Err` is the reason the import could not be stored, for the job's `CREATE FAILED`
/// state; nothing is imported then, and what the dataset already held is untouched —
/// which is why every row's entity id is resolved BEFORE the dataset's own records are
/// touched at all.
pub fn apply_import(
    dataset: &mut crate::state::DatasetRecord,
    rows: Vec<Row>,
    import_mode: &str,
) -> Result<usize, String> {
    let imported = rows.len();
    let appends = import_mode == crate::state::pipeline::INCREMENTAL_IMPORT_MODE;
    // BEFORE any per-row work, and before the retained rows are touched: a submission
    // past the ceiling costs nothing and loses nothing.
    let retained = if appends { dataset.imported.len() } else { 0 };
    if retained.saturating_add(imported) > MAX_DATASET_ROWS {
        return Err(format!(
            "Dataset {} would hold {} records, more than the {MAX_DATASET_ROWS} this emulator \
             keeps for one dataset.",
            dataset.dataset_arn,
            retained.saturating_add(imported)
        ));
    }

    match record_id_column(dataset) {
        Some(id_column) => {
            // Resolve the entity id of EVERY incoming row first. A row this reader
            // cannot place fails the whole import, and a failed import leaves the
            // dataset exactly as it was, so nothing is written until every row has one.
            let mut identified: Vec<(String, Row)> = Vec::with_capacity(imported);
            for row in rows {
                let Some(id) = column(&row, id_column).map(str::to_string) else {
                    return Err(format!(
                        "A record being imported into {} names no {id_column}.",
                        dataset.dataset_arn
                    ));
                };
                identified.push((id, row));
            }

            let mut records = if appends {
                std::mem::take(&mut dataset.imported)
            } else {
                Vec::new()
            };
            // One pass over what is stored indexes the entity ids, so the loop below is
            // one lookup per incoming row instead of a scan over everything stored.
            let mut index: HashMap<String, usize> = HashMap::with_capacity(records.len());
            for (position, stored) in records.iter().enumerate() {
                if let Some(id) = column(stored, id_column) {
                    index.insert(id.to_string(), position);
                }
            }
            for (id, row) in identified {
                match index.get(&id).copied() {
                    // "Amazon Personalize replaces any record with the same ID with the
                    // new one."
                    Some(position) => {
                        if let Some(stored) = records.get_mut(position) {
                            *stored = row;
                        }
                    }
                    None => {
                        index.insert(id, records.len());
                        records.push(row);
                    }
                }
            }
            dataset.imported = records;
        }
        // An interactions dataset holds a stream, where two rows with the same ids are
        // two interactions, so nothing is replaced and no row can fail to be placed.
        None => {
            if appends {
                dataset.imported.extend(rows);
            } else {
                dataset.imported = rows;
            }
        }
    }
    Ok(imported)
}
