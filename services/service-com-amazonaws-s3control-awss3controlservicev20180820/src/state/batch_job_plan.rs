// SPDX-License-Identifier: CC0-1.0

//! What a submitted S3 Batch Operations job's RUN reads and writes, as pure
//! derivations: the object list a manifest names, the object list a manifest
//! generator selects, and the objects a completed job leaves behind.
//!
//! Held apart from [`super::batch_job_worker`], which carries them out, because
//! none of this needs the store, the clock or Amazon S3: a manifest's rows are a
//! function of its bytes and the columns its `JobManifestSpec` declares, a
//! generated manifest's rows are a function of the source bucket's listing and
//! the generator's `Filter`, and a completion report is a function of what the
//! tasks did.
//!
//! The CSV a manifest is written in is read and written with the `csv` crate —
//! the format defines quoting, embedded separators and embedded newlines, and a
//! manifest is caller-supplied input — never by splitting on commas here.
//!
//! An object list is addressed by the ARN its `JobManifestLocation` carries;
//! the location's `ETag` is not what selects it.

use super::*;

use crate::ops::types::{
    JobManifestGeneratorFilter, JobManifestLocation, KeyNameConstraint,
    GENERATED_MANIFEST_FORMAT_VALUES,
};

/// One row of a job's object list: the object one task acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// The bucket holding the object, as the manifest's `Bucket` column names
    /// it.
    pub bucket: String,
    /// The object's key, as the manifest's `Key` column names it.
    pub key: String,
    /// The version the manifest's `VersionId` column names, if it carries that
    /// column — `None` leaves the task acting on the current version.
    pub version_id: Option<String>,
}

/// The `JobManifestFormat` a manifest written FOR a batch job carries, whose
/// columns `JobManifestSpec$Fields` describes ("if the specified manifest object
/// is in the `S3BatchOperations_CSV_20180820` format, this element describes
/// which columns contain the required data").
pub const MANIFEST_FORMAT_CSV: &str = "S3BatchOperations_CSV_20180820";

/// The `GeneratedManifestFormat` a manifest this service GENERATES is written
/// in — the single value that enum declares.
pub const GENERATED_MANIFEST_FORMAT: &str = "S3InventoryReport_CSV_20211130";

/// The `JobManifestFieldName` values a CSV manifest's columns are described
/// with: the model's own enum
/// (`smithy_query(query="shape",shape="JobManifestFieldName")`).
pub const FIELD_BUCKET: &str = "Bucket";
/// See [`FIELD_BUCKET`].
pub const FIELD_KEY: &str = "Key";
/// See [`FIELD_BUCKET`].
pub const FIELD_VERSION_ID: &str = "VersionId";

/// The columns a CSV manifest carries when its spec describes none: the bucket
/// and the key, which are the two a task needs to address an object.
pub const DEFAULT_MANIFEST_FIELDS: [&str; 2] = [FIELD_BUCKET, FIELD_KEY];

/// The `JobReportFormat` a completion report is written in — the single value
/// that enum declares.
pub const REPORT_FORMAT: &str = "Report_CSV_20180820";

/// The `JobReportScope` that puts ONLY failed tasks in the completion report
/// ("Indicates whether the job-completion report will include details of all
/// tasks or only failed tasks", `JobReport$ReportScope`).
pub const REPORT_SCOPE_FAILED_ONLY: &str = "FailedTasksOnly";

/// The columns of a completion report's result file as the report's own manifest
/// DECLARES them to a reader.
///
/// The declaration names `HTTPStatusCode` ahead of `ErrorCode`, while the result
/// file itself writes the task's error code in that first column and the HTTP
/// status in the second ([`report_results_csv`]) — so a reader that trusts the
/// declared order reads those two swapped. That is the real service's own
/// behaviour, recorded, and a reader of a completion report written anywhere
/// else depends on it; the declaration is therefore reproduced as it is rather
/// than corrected to the order the rows are in.
pub const REPORT_SCHEMA: &str =
    "Bucket, Key, VersionId, TaskStatus, HTTPStatusCode, ErrorCode, ResultMessage";

/// The `TaskStatus` of a task that was carried out, and of one that was not —
/// the two values a completion report's `TaskStatus` column carries, and the
/// `TaskExecutionStatus` the report's manifest reports over its results.
pub const TASK_SUCCEEDED: &str = "succeeded";
/// See [`TASK_SUCCEEDED`].
pub const TASK_FAILED: &str = "failed";

/// The `ResultMessage` a task that WAS carried out carries: a served task's
/// request was answered, so what there is to say about it is that it succeeded.
///
/// The column is filled for every row, served or refused, which is what leaves
/// the `ErrorCode` column as the one that is empty exactly when the task was
/// carried out — the distinction a reader of the report branches on.
pub const TASK_SUCCEEDED_MESSAGE: &str = "Successful";

/// The segment a directory bucket's ARN names the bucket after
/// (`arn:aws:s3express:<region>:<account>:/bucket/<name>`), built from the
/// resource type this crate already names for it.
const EXPRESS_BUCKET_SEGMENT_SEPARATOR: char = '/';

/// The bucket `bucket_arn` names, or `None` for a value that is no S3 bucket
/// ARN.
///
/// Both forms the batch surface addresses a bucket in: the general purpose
/// bucket's `arn:<partition>:s3:::<name>`, whose resource field IS the name, and
/// the directory bucket's `arn:<partition>:s3express:<region>:<account>:/bucket/<name>`.
/// Read through the workspace's one caller-supplied-ARN parse, so this module
/// does not split an ARN its own way.
pub fn arn_bucket(bucket_arn: &str) -> Option<String> {
    let arn = ::service_common::arn::parse(bucket_arn)?;
    match arn.vendor {
        S3_ARN_NAMESPACE => Some(arn.resource.to_string()),
        S3_EXPRESS_ARN_NAMESPACE => arn
            .resource
            .trim_start_matches(EXPRESS_BUCKET_SEGMENT_SEPARATOR)
            .strip_prefix(BUCKET_RESOURCE_TYPE)?
            .strip_prefix(EXPRESS_BUCKET_SEGMENT_SEPARATOR)
            .map(str::to_string),
        _ => None,
    }
}

/// The bucket and key `object_arn` names — a `JobManifestLocation$ObjectArn`,
/// "the Amazon Resource Name (ARN) for a manifest object" — or `None` for a
/// value that names no object.
pub fn arn_bucket_and_key(object_arn: &str) -> Option<(String, String)> {
    let bucket_and_key = arn_bucket(object_arn)?;
    let (bucket, key) = bucket_and_key.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// The rows `body` carries, read as a CSV manifest whose columns are `fields`
/// (or [`DEFAULT_MANIFEST_FIELDS`] when the spec describes none).
///
/// A row is one object: the `Bucket` and `Key` columns are what a task needs,
/// `VersionId` narrows it to one version, and `Ignore` is a column the manifest
/// carries for its own purposes. A row missing either required column, or
/// carrying an empty value in one, names no object — the manifest cannot be
/// read, rather than the job running over the rows that happen to parse.
///
/// BOUNDED: at most [`MAX_JOB_TASKS`] rows; a longer manifest is refused here
/// rather than run over in part.
pub fn parse_csv_manifest(body: &[u8], fields: &[String]) -> Result<Vec<ManifestEntry>, String> {
    let columns: Vec<&str> = if fields.is_empty() {
        DEFAULT_MANIFEST_FIELDS.to_vec()
    } else {
        fields.iter().map(String::as_str).collect()
    };
    let bucket_at = columns.iter().position(|field| *field == FIELD_BUCKET);
    let key_at = columns.iter().position(|field| *field == FIELD_KEY);
    let version_at = columns.iter().position(|field| *field == FIELD_VERSION_ID);
    let (Some(bucket_at), Some(key_at)) = (bucket_at, key_at) else {
        return Err(format!(
            "The manifest's Spec describes no {FIELD_BUCKET} and {FIELD_KEY} columns, so no \
             object can be read from it."
        ));
    };

    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        // A manifest row carries only the columns the spec describes, but a
        // reader that refuses a row of another width would refuse the whole
        // manifest over one trailing field: the columns this reads are the ones
        // the spec named, by position.
        .flexible(true)
        .from_reader(body);
    let mut entries = Vec::new();
    for (row, record) in reader.records().enumerate() {
        let record = record.map_err(|error| {
            format!(
                "The manifest is not readable as {MANIFEST_FORMAT_CSV} at row {}: {error}",
                row + 1
            )
        })?;
        // A manifest written with a trailing newline ends in an empty record,
        // which names no object and is not a row.
        if record.iter().all(|field| field.trim().is_empty()) {
            continue;
        }
        if entries.len() >= MAX_JOB_TASKS {
            return Err(format!(
                "This emulator runs at most {MAX_JOB_TASKS} tasks in one job; the manifest names \
                 more."
            ));
        }
        let column = |at: usize| record.get(at).map(str::trim).unwrap_or_default();
        let (bucket, key) = (column(bucket_at), column(key_at));
        if bucket.is_empty() || key.is_empty() {
            return Err(format!(
                "The manifest names no object at row {}: both {FIELD_BUCKET} and {FIELD_KEY} are \
                 required.",
                row + 1
            ));
        }
        entries.push(ManifestEntry {
            bucket: bucket.to_string(),
            key: key.to_string(),
            version_id: version_at
                .map(column)
                .filter(|version| !version.is_empty())
                .map(str::to_string),
        });
    }
    Ok(entries)
}

/// `entries` as the CSV body of a generated manifest: one row per object,
/// carrying the bucket and the key, in the order the listing selected them.
pub fn generated_manifest_csv(entries: &[ManifestEntry]) -> Vec<u8> {
    let mut writer = ::csv::WriterBuilder::new().from_writer(Vec::new());
    for entry in entries {
        // Writing to a `Vec` cannot fail, and the record carries no field the
        // writer can reject.
        let _ = writer.write_record([entry.bucket.as_str(), entry.key.as_str()]);
    }
    writer.into_inner().unwrap_or_default()
}

/// Whether the object `key` of `size` bytes, last modified at `modified_secs`
/// (epoch seconds), is one a generated manifest includes under `filter`.
///
/// Each constraint the filter carries NARROWS the listing, and an object has to
/// satisfy every one it carries:
///
/// * `KeyNameConstraint` — "the generated manifest includes only source bucket
///   objects whose object keys match the string constraints specified for
///   `MatchAnyPrefix`, `MatchAnySuffix`, and `MatchAnySubstring`", so a list the
///   constraint carries admits an object matching ANY of its strings and every
///   list it carries has to admit it;
/// * `ObjectSizeGreaterThanBytes` / `ObjectSizeLessThanBytes` — the size bounds,
///   exclusive, as "greater than" and "less than" state;
/// * `CreatedAfter` / `CreatedBefore` — the objects "created after this time" and
///   "created before this time", exclusive for the same reason;
/// * `MatchAnyStorageClass` — the storage classes the object may be in.
///
/// A filter carrying none of them selects the whole listing. The members this
/// service cannot answer from a bucket listing are NOT silently ignored: see
/// [`filter_members_not_applied`].
pub fn selected_by_filter(
    filter: Option<&JobManifestGeneratorFilter>,
    key: &str,
    size: i64,
    modified_secs: Option<i64>,
    storage_class: Option<&str>,
) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if let Some(constraint) = filter.key_name_constraint.as_ref() {
        if !key_matches(constraint, key) {
            return false;
        }
    }
    if filter
        .object_size_greater_than_bytes
        .is_some_and(|bound| size <= bound)
    {
        return false;
    }
    if filter
        .object_size_less_than_bytes
        .is_some_and(|bound| size >= bound)
    {
        return false;
    }
    if let Some(created_after) = filter.created_after.as_ref() {
        if modified_secs.is_none_or(|modified| modified <= created_after.secs()) {
            return false;
        }
    }
    if let Some(created_before) = filter.created_before.as_ref() {
        if modified_secs.is_none_or(|modified| modified >= created_before.secs()) {
            return false;
        }
    }
    if let Some(classes) = filter.match_any_storage_class.as_ref() {
        let class = storage_class.unwrap_or_default();
        if !classes.iter().any(|wanted| wanted == class) {
            return false;
        }
    }
    true
}

/// Whether `key` satisfies every list `constraint` carries — see
/// [`selected_by_filter`].
fn key_matches(constraint: &KeyNameConstraint, key: &str) -> bool {
    let prefixes = constraint
        .match_any_prefix
        .as_ref()
        .is_none_or(|values| values.iter().any(|value| key.starts_with(value)));
    let suffixes = constraint
        .match_any_suffix
        .as_ref()
        .is_none_or(|values| values.iter().any(|value| key.ends_with(value)));
    let substrings = constraint
        .match_any_substring
        .as_ref()
        .is_none_or(|values| values.iter().any(|value| key.contains(value)));
    prefixes && suffixes && substrings
}

/// The `Filter` members this service cannot decide from a bucket listing, for a
/// filter that carries them — the REPLICATION members, which are a bucket's
/// replication state rather than a property of the listed object, and the
/// ENCRYPTION members, which are the object's encryption configuration.
///
/// A filter carrying one of them selects a set this service cannot compute, so
/// the job it belongs to is failed naming the member rather than run over a
/// listing the filter did not narrow.
pub fn filter_members_not_applied(
    filter: Option<&JobManifestGeneratorFilter>,
) -> Vec<&'static str> {
    let Some(filter) = filter else {
        return Vec::new();
    };
    [
        (
            "EligibleForReplication",
            filter.eligible_for_replication.is_some(),
        ),
        (
            "ObjectReplicationStatuses",
            filter.object_replication_statuses.is_some(),
        ),
        (
            "MatchAnyObjectEncryption",
            filter.match_any_object_encryption.is_some(),
        ),
    ]
    .into_iter()
    .filter(|(_, carried)| *carried)
    .map(|(member, _)| member)
    .collect()
}

/// The key an object copied under `target_key_prefix` is written at.
///
/// "Specifies the folder prefix that you want the objects to be copied into. For
/// example, to copy objects into a folder named `Folder1` in the destination
/// bucket, set the `TargetKeyPrefix` property to `Folder1`"
/// (`S3CopyObjectOperation$TargetKeyPrefix`) — so the prefix is a FOLDER the
/// source key hangs under, whether or not it was written with a trailing
/// separator, and a copy with no prefix keeps the source key.
pub fn prefixed_key(target_key_prefix: Option<&str>, key: &str) -> String {
    match target_key_prefix.map(|prefix| prefix.trim_end_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{key}"),
        _ => key.to_string(),
    }
}

/// Where everything one job writes about ITSELF goes: `<prefix>/job-<job id>`,
/// the location a completion report is "stored" at under `JobReport$Prefix`
/// ("an optional prefix to describe where in the specified bucket the
/// job-completion report will be stored"), and the location a generated
/// manifest is written under `S3ManifestOutputLocation$ManifestPrefix`.
///
/// The job's own id is what keeps two jobs writing into one prefix apart.
pub fn job_prefix(prefix: Option<&str>, job_id: &str) -> String {
    match prefix.map(|prefix| prefix.trim_end_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/job-{job_id}"),
        _ => format!("job-{job_id}"),
    }
}

/// What one task did, as the completion report accounts for it.
#[derive(Debug, Clone)]
pub struct TaskOutcome {
    /// The object the task acted on.
    pub entry: ManifestEntry,
    /// Whether the task was carried out: [`TASK_SUCCEEDED`] or [`TASK_FAILED`].
    pub status: &'static str,
    /// The error code Amazon S3 answered the task's own request with, for a
    /// task that failed; empty for one that was carried out.
    pub error_code: String,
    /// The HTTP status that answer carried, or `None` for a request that
    /// reached no answer at all.
    pub http_status: Option<u16>,
    /// What there is to say about the outcome: the sentence of the answer that
    /// refused the task, or [`TASK_SUCCEEDED_MESSAGE`] for one that was carried
    /// out — a result message every row carries, which is what makes the error
    /// code the one column that tells a failed row from a served one.
    pub message: String,
}

impl TaskOutcome {
    /// Whether this task was carried out.
    pub fn succeeded(&self) -> bool {
        self.status == TASK_SUCCEEDED
    }
}

/// The result file of a completion report over `outcomes`, narrowed to the tasks
/// `report_scope` asks for: one row per task, carrying the object it acted on,
/// whether it was carried out, the error code and HTTP status of the answer it
/// got, and the result message.
///
/// The error code comes BEFORE the HTTP status, which is the order the real
/// service writes the two in — and not the order its own
/// [`REPORT_SCHEMA`] declares, as that constant explains.
pub fn report_results_csv(outcomes: &[TaskOutcome], report_scope: Option<&str>) -> Vec<u8> {
    let failed_only = report_scope == Some(REPORT_SCOPE_FAILED_ONLY);
    let mut writer = ::csv::WriterBuilder::new().from_writer(Vec::new());
    for outcome in outcomes
        .iter()
        .filter(|outcome| !failed_only || !outcome.succeeded())
    {
        let _ = writer.write_record([
            outcome.entry.bucket.as_str(),
            outcome.entry.key.as_str(),
            outcome.entry.version_id.as_deref().unwrap_or_default(),
            outcome.status,
            outcome.error_code.as_str(),
            &outcome
                .http_status
                .map(|status| status.to_string())
                .unwrap_or_default(),
            outcome.message.as_str(),
        ]);
    }
    writer.into_inner().unwrap_or_default()
}

/// The manifest of a completion report: the format it is written in, when it
/// was created, the schema its result file carries and that file's own
/// location, so a reader finds the results from the report's one fixed key.
///
/// `md5` is the result file's MD5 digest as Amazon S3 computed it for the
/// object — the `ETag` of the write that stored it — and the member is left out
/// when the write reported none, rather than carrying a digest nothing
/// computed.
pub fn report_manifest_json(
    report_bucket: &str,
    results_key: &str,
    md5: Option<&str>,
    created: ::aws_smithy_types::DateTime,
    succeeded: bool,
) -> Vec<u8> {
    let mut result = ::serde_json::Map::new();
    result.insert(
        "TaskExecutionStatus".to_string(),
        ::serde_json::Value::String(
            if succeeded { TASK_SUCCEEDED } else { TASK_FAILED }.to_string(),
        ),
    );
    result.insert(
        "Bucket".to_string(),
        ::serde_json::Value::String(report_bucket.to_string()),
    );
    result.insert(
        "Key".to_string(),
        ::serde_json::Value::String(results_key.to_string()),
    );
    if let Some(md5) = md5 {
        result.insert(
            "MD5Checksum".to_string(),
            ::serde_json::Value::String(md5.to_string()),
        );
    }
    let document = ::serde_json::json!({
        "Format": REPORT_FORMAT,
        "ReportCreationDate": created
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default(),
        "Results": [::serde_json::Value::Object(result)],
        "ReportSchema": REPORT_SCHEMA,
    });
    ::serde_json::to_vec_pretty(&document).unwrap_or_default()
}

/// `GENERATED_MANIFEST_FORMAT` as the model's own enum spells it — the value
/// written into a job's `GeneratedManifestDescriptor` and into the manifest
/// output location of the manifest this service wrote.
pub fn generated_manifest_format() -> String {
    modelled_value(GENERATED_MANIFEST_FORMAT_VALUES, GENERATED_MANIFEST_FORMAT)
}

/// The manifest of a GENERATED manifest, in the form its format names.
///
/// `GeneratedManifestFormat` has the single value
/// [`GENERATED_MANIFEST_FORMAT`] — the S3 Inventory report format — whose
/// manifest object is a `manifest.json` describing the CSV data file beside it:
/// the bucket the objects came from, the bucket it was written to, the format
/// version the value names, when it was created, the CSV's own column schema and
/// the file itself with its size and the digest Amazon S3 computed for it. A
/// reader of a manifest in that format starts from this object, which is why it
/// is the one the job's `GeneratedManifestDescriptor` locates.
///
/// `md5` is the data file's `ETag` as the write that stored it reported it, and
/// the member is left out when the write reported none, rather than carrying a
/// digest nothing computed.
pub fn inventory_manifest_json(
    source_bucket: &str,
    destination_bucket: &str,
    data_key: &str,
    data_size: usize,
    md5: Option<&str>,
    created: ::aws_smithy_types::DateTime,
) -> Vec<u8> {
    let mut file = ::serde_json::Map::new();
    file.insert(
        "key".to_string(),
        ::serde_json::Value::String(data_key.to_string()),
    );
    file.insert("size".to_string(), ::serde_json::json!(data_size));
    if let Some(md5) = md5 {
        file.insert(
            "MD5checksum".to_string(),
            ::serde_json::Value::String(md5.to_string()),
        );
    }
    let document = ::serde_json::json!({
        "sourceBucket": source_bucket,
        "destinationBucket": format!("arn:aws:{S3_ARN_NAMESPACE}:::{destination_bucket}"),
        "version": GENERATED_MANIFEST_VERSION,
        "creationTimestamp": created.to_millis().unwrap_or_default().to_string(),
        "fileFormat": GENERATED_MANIFEST_FILE_FORMAT,
        "fileSchema": GENERATED_MANIFEST_SCHEMA,
        "files": [::serde_json::Value::Object(file)],
    });
    ::serde_json::to_vec_pretty(&document).unwrap_or_default()
}

/// The format version a manifest in [`GENERATED_MANIFEST_FORMAT`] names: the
/// date that value carries.
const GENERATED_MANIFEST_VERSION: &str = "2021-11-30";

/// The file format the data file of a generated manifest is written in — the
/// `CSV` the format's own value names.
const GENERATED_MANIFEST_FILE_FORMAT: &str = "CSV";

/// The columns the data file of a generated manifest carries, in order: what
/// [`generated_manifest_csv`] writes, and what a job reading the manifest back
/// reads it by.
const GENERATED_MANIFEST_SCHEMA: &str = "Bucket, Key";

/// The location a manifest object this service WROTE is reported at, as
/// `JobManifestLocation` carries it: the object's ARN and the `ETag` the write
/// that stored it answered.
pub fn written_manifest_location(
    bucket: &str,
    key: &str,
    e_tag: Option<&str>,
) -> JobManifestLocation {
    JobManifestLocation {
        object_arn: format!("arn:aws:{S3_ARN_NAMESPACE}:::{bucket}/{key}"),
        object_version_id: None,
        e_tag: e_tag.unwrap_or_default().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row a manifest must be read as: its bucket, its key and the version
    /// it names (empty for a row that names none).
    type Row = (&'static str, &'static str, &'static str);

    /// One row of the manifest-reading table: the case name, the manifest's
    /// bytes, the columns its spec describes, and the rows it must be read as —
    /// `None` for a manifest that cannot be read at all.
    type ManifestCase = (
        &'static str,
        &'static [u8],
        &'static [&'static str],
        Option<&'static [Row]>,
    );

    /// One row of the object-ARN table: the case name, the value, and the
    /// (bucket, key) pair it names — `None` for a value that names no object.
    type ObjectArnCase = (
        &'static str,
        &'static str,
        Option<(&'static str, &'static str)>,
    );

    /// One row of the report-scope table: the case name, the scope, the keys the
    /// report must account for and the keys it must leave out.
    type ReportScopeCase = (
        &'static str,
        Option<&'static str>,
        &'static [&'static str],
        &'static [&'static str],
    );

    /// A manifest's rows are read by the COLUMNS its spec describes, whatever
    /// order they are in and whatever quoting the CSV uses — and a manifest
    /// naming no object in a row is not readable at all.
    #[test]
    fn a_csv_manifest_is_read_by_the_columns_its_spec_describes() {
        let cases: &[ManifestCase] = &[
            (
                "the default columns",
                b"reports,finance/q1.csv\nreports,finance/q2.csv\n",
                &[],
                Some(&[
                    ("reports", "finance/q1.csv", ""),
                    ("reports", "finance/q2.csv", ""),
                ]),
            ),
            (
                "a version column",
                b"reports,finance/q1.csv,v7\n",
                &["Bucket", "Key", "VersionId"],
                Some(&[("reports", "finance/q1.csv", "v7")]),
            ),
            (
                "columns in another order, with one ignored",
                b"skip-me,finance/q1.csv,reports\n",
                &["Ignore", "Key", "Bucket"],
                Some(&[("reports", "finance/q1.csv", "")]),
            ),
            (
                "a quoted key carrying the separator",
                b"reports,\"finance,q1.csv\"\n",
                &[],
                Some(&[("reports", "finance,q1.csv", "")]),
            ),
            (
                "no trailing newline",
                b"reports,finance/q1.csv",
                &[],
                Some(&[("reports", "finance/q1.csv", "")]),
            ),
            ("an empty manifest", b"", &[], Some(&[])),
            ("a row naming no key", b"reports,\n", &[], None),
            (
                "a spec describing neither required column",
                b"reports,finance/q1.csv\n",
                &["Ignore", "Ignore"],
                None,
            ),
        ];
        for (case, body, fields, expected) in cases {
            let fields: Vec<String> = fields.iter().map(|field| (*field).to_string()).collect();
            let parsed = parse_csv_manifest(body, &fields);
            match expected {
                Some(rows) => {
                    let entries = parsed.unwrap_or_else(|error| {
                        panic!("case {case:?}: expected the manifest to be read, got {error}")
                    });
                    let actual: Vec<(&str, &str, &str)> = entries
                        .iter()
                        .map(|entry| {
                            (
                                entry.bucket.as_str(),
                                entry.key.as_str(),
                                entry.version_id.as_deref().unwrap_or_default(),
                            )
                        })
                        .collect();
                    assert_eq!(
                        actual, *rows,
                        "case {case:?}: expected the rows {rows:?}, got {actual:?}"
                    );
                }
                None => assert!(
                    parsed.is_err(),
                    "case {case:?}: expected the manifest to be unreadable, got {parsed:?}"
                ),
            }
        }
    }

    /// Every bound a generator's filter states narrows the listing, and an
    /// object has to satisfy all of them.
    #[test]
    fn a_generator_filter_narrows_the_listing_by_every_bound_it_states() {
        let constraint = |prefixes: &[&str], suffixes: &[&str], substrings: &[&str]| {
            KeyNameConstraint {
                match_any_prefix: (!prefixes.is_empty())
                    .then(|| prefixes.iter().map(|value| (*value).to_string()).collect()),
                match_any_suffix: (!suffixes.is_empty())
                    .then(|| suffixes.iter().map(|value| (*value).to_string()).collect()),
                match_any_substring: (!substrings.is_empty()).then(|| {
                    substrings
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect()
                }),
            }
        };
        let cases: &[(&str, JobManifestGeneratorFilter, &str, i64, bool)] = &[
            (
                "a prefix the key starts with",
                JobManifestGeneratorFilter {
                    key_name_constraint: Some(constraint(&["logs/"], &[], &[])),
                    ..JobManifestGeneratorFilter::default()
                },
                "logs/today.txt",
                10,
                true,
            ),
            (
                "a prefix the key does not start with",
                JobManifestGeneratorFilter {
                    key_name_constraint: Some(constraint(&["logs/"], &[], &[])),
                    ..JobManifestGeneratorFilter::default()
                },
                "metrics/today.txt",
                10,
                false,
            ),
            (
                "any of two prefixes",
                JobManifestGeneratorFilter {
                    key_name_constraint: Some(constraint(&["logs/", "metrics/"], &[], &[])),
                    ..JobManifestGeneratorFilter::default()
                },
                "metrics/today.txt",
                10,
                true,
            ),
            (
                "a suffix and a prefix together",
                JobManifestGeneratorFilter {
                    key_name_constraint: Some(constraint(&["logs/"], &[".txt"], &[])),
                    ..JobManifestGeneratorFilter::default()
                },
                "logs/today.csv",
                10,
                false,
            ),
            (
                "a substring anywhere in the key",
                JobManifestGeneratorFilter {
                    key_name_constraint: Some(constraint(&[], &[], &["-eu-"])),
                    ..JobManifestGeneratorFilter::default()
                },
                "logs/region-eu-west.txt",
                10,
                true,
            ),
            (
                "a size bound the object is exactly at",
                JobManifestGeneratorFilter {
                    object_size_greater_than_bytes: Some(10),
                    ..JobManifestGeneratorFilter::default()
                },
                "logs/today.txt",
                10,
                false,
            ),
            (
                "a size bound the object is over",
                JobManifestGeneratorFilter {
                    object_size_greater_than_bytes: Some(10),
                    ..JobManifestGeneratorFilter::default()
                },
                "logs/today.txt",
                11,
                true,
            ),
            (
                "an upper size bound the object is under",
                JobManifestGeneratorFilter {
                    object_size_less_than_bytes: Some(10),
                    ..JobManifestGeneratorFilter::default()
                },
                "logs/today.txt",
                9,
                true,
            ),
            (
                "no bound at all",
                JobManifestGeneratorFilter::default(),
                "anything",
                0,
                true,
            ),
        ];
        for (case, filter, key, size, expected) in cases {
            let actual = selected_by_filter(Some(filter), key, *size, Some(1_000), Some("STANDARD"));
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the object {key:?} of {size} bytes to be selected \
                 {expected}, got {actual}"
            );
        }
    }

    /// A copy's target key hangs the source key under the target prefix, with
    /// one separator however the prefix was written.
    #[test]
    fn a_target_prefix_is_the_folder_the_source_key_hangs_under() {
        let cases: &[(&str, Option<&str>, &str, &str)] = &[
            ("no prefix", None, "finance/q1.csv", "finance/q1.csv"),
            (
                "a bare folder",
                Some("copies"),
                "finance/q1.csv",
                "copies/finance/q1.csv",
            ),
            (
                "a folder written with a separator",
                Some("copies/"),
                "finance/q1.csv",
                "copies/finance/q1.csv",
            ),
            (
                "a nested folder",
                Some("copies/2026/"),
                "q1.csv",
                "copies/2026/q1.csv",
            ),
            ("an empty prefix", Some(""), "q1.csv", "q1.csv"),
        ];
        for (case, prefix, key, expected) in cases {
            let actual = prefixed_key(*prefix, key);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the copied key {expected:?}, got {actual:?}"
            );
        }
    }

    /// A bucket or object ARN names the bucket and key a task addresses, in
    /// both namespaces the batch surface admits.
    #[test]
    fn an_s3_arn_names_the_bucket_and_the_key() {
        let buckets: &[(&str, &str, Option<&str>)] = &[
            ("a general purpose bucket", "arn:aws:s3:::reports", Some("reports")),
            (
                "a directory bucket",
                "arn:aws:s3express:us-west-2:123456789012:/bucket/reports--usw2-az1--x-s3",
                Some("reports--usw2-az1--x-s3"),
            ),
            ("another vendor's ARN", "arn:aws:sqs:us-west-2:123456789012:queue", None),
            ("not an ARN at all", "reports", None),
        ];
        for (case, value, expected) in buckets {
            let actual = arn_bucket(value);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected the bucket {expected:?}, got {actual:?}"
            );
        }

        let objects: &[ObjectArnCase] = &[
            (
                "an object ARN",
                "arn:aws:s3:::manifests/objects.csv",
                Some(("manifests", "objects.csv")),
            ),
            (
                "a key with separators of its own",
                "arn:aws:s3:::manifests/2026/q1/objects.csv",
                Some(("manifests", "2026/q1/objects.csv")),
            ),
            ("a bucket ARN with no key", "arn:aws:s3:::manifests", None),
            ("an empty key", "arn:aws:s3:::manifests/", None),
        ];
        for (case, value, expected) in objects {
            let actual = arn_bucket_and_key(value);
            let actual_pair = actual
                .as_ref()
                .map(|(bucket, key)| (bucket.as_str(), key.as_str()));
            assert_eq!(
                actual_pair, *expected,
                "case {case:?}: expected {expected:?}, got {actual_pair:?}"
            );
        }
    }

    /// A completion report's result file carries one row per task in schema
    /// order, and `FailedTasksOnly` leaves out the tasks that were carried out.
    #[test]
    fn a_completion_report_accounts_for_the_tasks_its_scope_asks_for() {
        let outcomes = vec![
            TaskOutcome {
                entry: ManifestEntry {
                    bucket: "reports".to_string(),
                    key: "q1.csv".to_string(),
                    version_id: None,
                },
                status: TASK_SUCCEEDED,
                error_code: String::new(),
                http_status: Some(200),
                message: String::new(),
            },
            TaskOutcome {
                entry: ManifestEntry {
                    bucket: "reports".to_string(),
                    key: "absent.csv".to_string(),
                    version_id: Some("v2".to_string()),
                },
                status: TASK_FAILED,
                error_code: "NoSuchKey".to_string(),
                http_status: Some(404),
                message: "The specified key does not exist.".to_string(),
            },
        ];
        let cases: &[ReportScopeCase] = &[
            (
                "every task",
                Some("AllTasks"),
                &["q1.csv", "absent.csv"],
                &[],
            ),
            (
                "failed tasks only",
                Some(REPORT_SCOPE_FAILED_ONLY),
                &["absent.csv"],
                &["q1.csv"],
            ),
            ("no scope stated", None, &["q1.csv", "absent.csv"], &[]),
        ];
        for (case, scope, carried, left_out) in cases {
            let rendered = String::from_utf8(report_results_csv(&outcomes, *scope))
                .unwrap_or_else(|error| panic!("case {case:?}: the report is UTF-8: {error}"));
            for key in *carried {
                assert!(
                    rendered.contains(key),
                    "case {case:?}: expected the report to account for {key:?}, got {rendered:?}"
                );
            }
            for key in *left_out {
                assert!(
                    !rendered.contains(key),
                    "case {case:?}: expected the report to leave out {key:?}, got {rendered:?}"
                );
            }
        }
        let failed_row = String::from_utf8(report_results_csv(&outcomes, None)).unwrap_or_default();
        assert!(
            failed_row.contains("reports,absent.csv,v2,failed,NoSuchKey,404,"),
            "expected the failed task's row to carry the error code before the HTTP status, got \
             {failed_row:?}"
        );
    }
}
