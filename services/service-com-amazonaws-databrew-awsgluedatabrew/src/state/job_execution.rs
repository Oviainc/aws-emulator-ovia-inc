// SPDX-License-Identifier: CC0-1.0

//! What a recipe-job RUN does: read the columns of the dataset the job names, apply the
//! steps of the recipe version it references to them, and decide the terminal state that
//! pair is owed.
//!
//! A DataBrew job run is admitted first and executes afterwards — `StartJobRun` answers
//! `RunId` and the run reaches a terminal state on its own — so a recipe that does not fit
//! its dataset is never a refusal at the door: the run is admitted, runs, and ends
//! `FAILED` carrying the diagnosis of the step that could not be applied. That diagnosis
//! is only possible if something really reads the data, so the dataset's columns come from
//! the OBJECT Amazon S3 holds at the dataset's own `S3InputDefinition`, read through the
//! operation callback ([`smithy_server_runtime::OperationContext::callback`]), which routes
//! IN PROCESS: no socket, no outbound request, no host resolution. A header a caller
//! staged with `PutObject` is therefore the header the run is judged against.
//!
//! ## What is established, and what is not
//!
//! A step is reported as unapplicable ONLY against a column list this emulator really
//! read. When the dataset names no S3 object, the object is not there, the format is not
//! one whose column names live in the data, or the read answers nothing at all, the column
//! list is UNESTABLISHED ([`dataset_columns`] answers `None`) and no step is judged: the
//! run is owed the state it was owed before, because a missing column is a claim about
//! data this emulator did not see.
//!
//! HOW MUCH IS READ IS BOUNDED: the read asks Amazon S3 for the first
//! [`MAX_HEADER_BYTES`] of the object and nothing more, so the bytes taken into this
//! process do not scale with the dataset, and a header naming more than
//! [`MAX_COLUMNS`] columns is left unestablished rather than held.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use crate::ops::types::RecipeStep;

use super::DatasetRecord;

/// The most bytes of a dataset object this emulator reads to find its column names. A
/// header is the FIRST record of the object, so the read asks Amazon S3 for a prefix of
/// it rather than for the whole dataset, and the bytes that enter this process are
/// bounded before any of them do.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// The most columns this emulator holds for one dataset, so neither the column list nor
/// the diagnosis built from it is sized by what a caller staged. A header naming more is
/// left unestablished rather than truncated: a truncated list would report a column that
/// IS present as missing.
pub const MAX_COLUMNS: usize = 1_000;

/// The dataset format whose column names are carried by the data itself, in its first
/// record (model: `CsvOptions$HeaderRow` — "A variable that specifies whether the first row
/// in the file is parsed as the header"). The other formats DataBrew reads
/// (model: `InputFormat`) carry their column names in a schema this emulator does not
/// hold, so a dataset in one of them establishes no column list here.
const CSV_FORMAT: &str = "CSV";

/// The file extension a dataset whose `Format` the create left out is read as CSV by:
/// `Format` is optional (model: `CreateDatasetRequest$Format`), and the format of such a
/// dataset follows from its object's name.
const CSV_EXTENSION: &str = ".csv";

/// The delimiter a CSV dataset is read with when its `FormatOptions` name none (model:
/// `CsvOptions$Delimiter` — "A single character that specifies the delimiter being used in
/// the CSV file"; a comma-separated value file is separated by a comma).
const DEFAULT_CSV_DELIMITER: u8 = b',';

/// The byte order mark a UTF-8 encoded text file may open with, which is not part of the
/// first column's name.
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// The recipe-action parameters that name a column the data must ALREADY have: the column
/// the transformation reads (model: `RecipeAction$Parameters` — "Contextual parameters for
/// the transformation" — whose names are the recipe structure
/// `ConditionExpression$Condition` points at). A parameter naming the column a step WRITES
/// is not one of them ([`TARGET_COLUMN_PARAMETER`]).
const SOURCE_COLUMN_PARAMETERS: [&str; 3] = ["sourceColumn", "sourceColumn1", "sourceColumn2"];

/// The recipe-action parameter naming SEVERAL such columns at once, as a JSON array of
/// names.
const SOURCE_COLUMNS_PARAMETER: &str = "sourceColumns";

/// The recipe-action parameter naming the column a step WRITES. It is not required to
/// exist beforehand — a step that renames or derives a column names the result here — so
/// after that step the name IS one of the data's columns.
const TARGET_COLUMN_PARAMETER: &str = "targetColumn";

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the object
/// a dataset names. Every other answer is this emulator failing to ask its own object
/// store, which establishes nothing either; the two are kept apart only so the second is
/// logged as the assembly fault it is.
const UNREADABLE_OBJECT_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    "NotFound",
    "AccessDenied",
    "AllAccessDisabled",
];

/// The terminal state a run is owed once it has executed, decided from the dataset and the
/// recipe version the run was admitted with.
///
/// The decision is made when the run is ADMITTED, against the data and the recipe version
/// as they were then, and materialised by the clock
/// ([`super::JobRunRecord::advance_to`]): a run reports the outcome of what it ran with,
/// not of what the dataset or the recipe became afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RunOutcome {
    /// The recipe applies to the data: the run ends in the terminal `SUCCEEDED` state and
    /// reports no `ErrorMessage`.
    #[default]
    Succeeded,
    /// The recipe cannot be applied: the run ends in the terminal `FAILED` state and
    /// reports this `ErrorMessage`, and the work it would have written is not written.
    Failed(String),
}

/// The `ErrorMessage` a run that could not apply one of its recipe's steps reports.
///
/// `step` is the 1-based index of the step that could not be applied, `column` the column
/// that step names and the data does not have, and `columns` the columns the data really
/// has, in the order the data carries them — the three facts an operator needs to repair
/// either the recipe or the dataset.
pub fn recipe_step_error_message(step: usize, column: &str, columns: &[String]) -> String {
    format!(
        "RecipeStepError: Failed at step {step}: Column '{column}' does not exist. \
         The dataset's columns are: {}",
        columns.join(", ")
    )
}

/// The terminal state the recipe `steps` are owed against the data columns `columns`.
///
/// The steps are applied IN ORDER against the columns the data has at that point in the
/// recipe: the first step that names a column which is not among them is the step that
/// cannot be applied, and the run is owed `FAILED` carrying its diagnosis. A step that
/// writes a column adds that name for the steps after it
/// ([`TARGET_COLUMN_PARAMETER`]), so a recipe that derives a column and then transforms it
/// applies.
///
/// Membership is decided through a set, so the walk costs no more than the size of the
/// recipe the create already stored however many columns or steps either side has.
pub fn apply_steps(steps: &[RecipeStep], columns: &[String]) -> RunOutcome {
    let mut present: ::std::collections::BTreeSet<String> = columns.iter().cloned().collect();
    for (index, step) in steps.iter().enumerate() {
        for referenced in referenced_columns(step) {
            if !present.contains(&referenced) {
                return RunOutcome::Failed(recipe_step_error_message(
                    index + 1,
                    &referenced,
                    columns,
                ));
            }
        }
        if let Some(written) = step
            .action
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.get(TARGET_COLUMN_PARAMETER))
            .filter(|written| !written.trim().is_empty())
        {
            present.insert(written.clone());
        }
    }
    RunOutcome::Succeeded
}

/// The columns one recipe step names and the data must already have, in a stable order:
/// the action's own source parameters, then the names its `sourceColumns` array carries,
/// then the column each of its conditions applies to (model:
/// `ConditionExpression$TargetColumn` — "A column to apply this condition to").
///
/// The order is fixed so the step's diagnosis names the same column on every run of the
/// same recipe.
fn referenced_columns(step: &RecipeStep) -> Vec<String> {
    let mut referenced = Vec::new();
    if let Some(parameters) = step.action.parameters.as_ref() {
        for name in SOURCE_COLUMN_PARAMETERS {
            if let Some(column) = parameters.get(name).filter(|value| !value.is_empty()) {
                referenced.push(column.clone());
            }
        }
        if let Some(list) = parameters.get(SOURCE_COLUMNS_PARAMETER) {
            referenced.extend(column_name_list(list));
        }
    }
    for condition in step.condition_expressions.iter().flatten() {
        referenced.push(condition.target_column.clone());
    }
    referenced
}

/// The column names a `sourceColumns` parameter carries. The value is a parameter string
/// (model: `ParameterMap`'s value is a string), and a recipe states several columns in it
/// as a JSON array of names; a value that is not such an array names no column list and is
/// read as naming nothing.
fn column_name_list(value: &str) -> Vec<String> {
    ::serde_json::from_str::<Vec<String>>(value).unwrap_or_default()
}

/// The columns the dataset `dataset` has, or `None` when this emulator established none of
/// them (see the module docs).
pub async fn dataset_columns(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    dataset: &DatasetRecord,
) -> Option<Vec<String>> {
    let object = dataset.input.s3_input_definition.as_ref()?;
    let key = object.key.as_deref().filter(|key| !key.is_empty())?;
    if !reads_as_csv(dataset, key) {
        return None;
    }
    // "If this value is false, column names are auto-generated" (model:
    // `CsvOptions$HeaderRow`) — a generated name is not one the data carries, so such a
    // dataset establishes no column list here.
    let csv = dataset
        .format_options
        .as_ref()
        .and_then(|options| options.csv.as_ref());
    if csv.and_then(|csv| csv.header_row) == Some(false) {
        return None;
    }
    let delimiter = csv
        .and_then(|csv| csv.delimiter.as_deref())
        .and_then(|delimiter| delimiter.bytes().next())
        .unwrap_or(DEFAULT_CSV_DELIMITER);

    let region = super::request_region(ctx);
    let head = object_prefix(&s3_client(ctx, &region), &object.bucket, key).await?;
    header_columns(&head, delimiter)
}

/// Whether `dataset` is read as a CSV file: either its `Format` says so, or it names no
/// format and its object's name does.
fn reads_as_csv(dataset: &DatasetRecord, key: &str) -> bool {
    match dataset.format.as_deref() {
        Some(format) => format.eq_ignore_ascii_case(CSV_FORMAT),
        None => key.to_ascii_lowercase().ends_with(CSV_EXTENSION),
    }
}

/// The column names the first record of `head` carries, read with `delimiter`, or `None`
/// when it carries none this emulator can hold.
///
/// `head` is a PREFIX of the object ([`MAX_HEADER_BYTES`]), so a header record longer than
/// that prefix was not seen to its end — the prefix would yield a clipped last column name,
/// which would then be reported as a column the data does not have. Such a header
/// establishes nothing: it is recognised by a prefix that fills the cap and carries no
/// record terminator at all.
fn header_columns(head: &[u8], delimiter: u8) -> Option<Vec<String>> {
    if head.len() >= MAX_HEADER_BYTES && !head.contains(&b'\n') {
        return None;
    }
    let head = head.strip_prefix(&UTF8_BOM).unwrap_or(head);
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(true)
        .from_reader(head);
    let columns: Vec<String> = reader
        .headers()
        .ok()?
        .iter()
        .map(|column| column.trim().to_string())
        .collect();
    if columns.is_empty() || columns.iter().all(String::is_empty) || columns.len() > MAX_COLUMNS {
        return None;
    }
    Some(columns)
}

/// The first [`MAX_HEADER_BYTES`] of the object `bucket`/`key`, or `None` when the store
/// did not serve them.
async fn object_prefix(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Option<::bytes::Bytes> {
    // The read asks for a prefix of the object rather than the object, so the bytes taken
    // into this process are bounded before any of them are.
    let read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(format!("bytes=0-{}", MAX_HEADER_BYTES - 1))
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            let code = error.code().unwrap_or_default();
            if !UNREADABLE_OBJECT_CODES.contains(&code) {
                // Not the store reporting that it holds no such object, but this emulator
                // failing to ask its own object store — worth a warning, because a
                // deployment with no Amazon S3 mounted to ask is one where no dataset's
                // columns are ever established. The caller's own values are rendered in
                // their escaped `Debug` form, so a bucket or key carrying a newline
                // cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    code = ?error.code(),
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a DataBrew dataset could not be read from the emulator's Amazon S3"
                );
            }
            return None;
        }
    };
    match read.body.collect().await {
        Ok(body) => {
            // The `Range` above is what bounds the read; the slice bounds what is HELD,
            // so an answer that ignored the range cannot grow the column list either.
            let bytes = body.into_bytes();
            let held = bytes.len().min(MAX_HEADER_BYTES);
            Some(bytes.slice(..held))
        }
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                error = %error,
                "a DataBrew dataset's bytes did not arrive from the emulator's Amazon S3"
            );
            None
        }
    }
}

/// The Amazon S3 client a dataset object is read with, addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is
/// an ADDRESS the in-process router reads rather than a host any resolver looks up: this
/// client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or key a
/// dataset names. The one caller-supplied value that reaches its authority is the signing
/// region, and [`super::request_region`] takes that from the request's own SigV4 credential
/// scope and otherwise answers the default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
