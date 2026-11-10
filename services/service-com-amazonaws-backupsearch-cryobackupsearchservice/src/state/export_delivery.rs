// SPDX-License-Identifier: CC0-1.0

//! Writing a search job's RESULTS into the destination bucket an export job names.
//!
//! # The rule this module implements
//!
//! `StartSearchResultExportJob` "starts a job to export the results of search job to a
//! designated S3 bucket", and the destination is the export's own
//! `ExportSpecification$s3ExportSpecification` — a `DestinationBucket` (`@required`) and
//! an optional `DestinationPrefix`. An export is therefore not a record of intent: it
//! produces an OBJECT, and that object belongs to another service. So the work here is a
//! write into that service, made the way every emulated service reaches another — the AWS
//! SDK driven over the emulator's own in-process transport
//! ([`smithy_server_runtime::OperationContext::callback`], through this tree's single
//! construction of the object-store client, [`service_common::s3_client`]) — and the status
//! the export settles to is what THAT write came to. Nothing here opens a socket or leaves
//! the host, and the only caller-supplied value that reaches the client's authority is the
//! signing Region, narrowed before it gets there ([`service_common::arn::region_label`]).
//!
//! Two consequences a client can observe follow from that, and they are the point of
//! writing it this way:
//!
//! * the destination bucket is RESOLVED where the object store holds it, so an export
//!   addressed at a bucket nothing holds does not settle as one addressed at a held bucket:
//!   the write is refused, and the export carries that refusal into the modelled `FAILED`
//!   status with the reason the object store gave ([`ExportDelivery::Failed`]);
//! * an export the object store accepted leaves the results object behind, so a client that
//!   exports and then reads that key — or lists the prefix — finds exactly the result items
//!   the search job recorded.
//!
//! # What is written, and where
//!
//! The results of ONE search job, as the CSV a consumer of an export reads: a header row of
//! the member names the model gives a `ResultItem`, then one row per item the search job
//! recorded ([`export_results_csv`]). The framing comes from the `csv` crate, so a value
//! carrying a separator, a quote or a newline is quoted as RFC 4180 requires rather than by
//! a hand-rolled writer.
//!
//! The key is the destination prefix followed by the export job's own identifier and the
//! suffix of that format ([`export_object_key`]). `DestinationPrefix` is "the prefix for
//! the destination Amazon S3 bucket for the export job", so it is used as exactly that —
//! the start of the key, verbatim, which is what makes a listing under the prefix a client
//! submitted find the object. The export job's identifier is what follows it, so two
//! exports into one prefix are two objects rather than one overwriting the other.
//!
//! # What one export costs
//!
//! BOUNDED by what the store already holds: an export makes exactly ONE write, of ONE
//! object, framed from the result items ONE search job recorded — a result set the scan's
//! own store caps, because it is the items of the recovery points this account holds. There
//! is no loop over the clock, no retry (the in-process client disables them) and no work
//! sized by anything the request carries beyond the destination it names. The number of
//! exports is itself capped by the store that holds them
//! ([`crate::state::MAX_EXPORT_JOBS`]).
//!
//! The destination a caller names is a BUCKET NAME and a key prefix — never a host, a URL
//! or a port. The client's endpoint is built from the Region alone, and the transport is the
//! in-process callback, so the write reaches this emulator's own object store and nothing
//! else, whatever a request names.

use ::smithy_server_runtime::EmbeddedEmulator;

use crate::ops::types::{EBSResultItem, ExportSpecification, ResultItem, S3ResultItem};

/// What the name of an exported results object ends with: the format its rows are written
/// in.
pub const EXPORT_OBJECT_SUFFIX: &str = ".csv";

/// The media type the results object is stored with, which is the format above.
const EXPORT_MEDIA_TYPE: &str = "text/csv";

/// The column naming WHICH kind of result item a row carries, which is the member of the
/// `ResultItem` union the item came from ([`S3_ITEM_TYPE`], [`EBS_ITEM_TYPE`]).
pub const ITEM_TYPE_COLUMN: &str = "ItemType";

/// The `ResultItem` union member an Amazon S3 search result is, as the model names it
/// (`smithy_query(query="shape", shape="ResultItem")`).
pub const S3_ITEM_TYPE: &str = "S3ResultItem";

/// The `ResultItem` union member an Amazon EBS search result is; see [`S3_ITEM_TYPE`].
pub const EBS_ITEM_TYPE: &str = "EBSResultItem";

/// The columns one exported row carries, in order: which member of the union the item is,
/// then every member of the two item shapes, the ones they share first.
///
/// Catalogue source: the members of `S3ResultItem` and `EBSResultItem` as the model
/// declares them (`smithy_query(query="shape", shape="S3ResultItem,EBSResultItem")`) —
/// `BackupResourceArn`, `SourceResourceArn` and `BackupVaultName` on both, then the Amazon
/// S3 item's own `ObjectKey`, `ObjectSize`, `ETag` and `VersionId`, then the Amazon EBS
/// item's own `FileSystemIdentifier`, `FilePath` and `FileSize`, then the times
/// (`CreationTime` on both, `LastModifiedTime` on the EBS item). ONE header for both item
/// types, so a result set holding either kind — or both — is one table, and a member the
/// row's own item shape does not declare is an empty cell.
pub const EXPORT_RESULT_COLUMNS: &[&str] = &[
    ITEM_TYPE_COLUMN,
    "BackupResourceArn",
    "SourceResourceArn",
    "BackupVaultName",
    "ObjectKey",
    "ObjectSize",
    "ETag",
    "VersionId",
    "FileSystemIdentifier",
    "FilePath",
    "FileSize",
    "CreationTime",
    "LastModifiedTime",
];

/// What writing an export's results into the destination bucket came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportDelivery {
    /// The object store took the object and holds it under this key, so the export has
    /// nothing left to do and the clock owes it [`crate::state::COMPLETED`].
    Written(String),
    /// Nothing was written, and this says why — the reason becomes the export job's
    /// `StatusMessage` beside the modelled [`crate::state::FAILED`] status, so no export
    /// reports a transfer the destination did not take.
    Failed(String),
}

/// The key the results object of the export `export_job_identifier` takes under `prefix`.
///
/// `prefix` is `DestinationPrefix` as the request spelled it, used verbatim as the start of
/// the key; an export that carries none writes at the bucket's root. See the module docs
/// for why the identifier follows it.
#[must_use]
pub fn export_object_key(prefix: Option<&str>, export_job_identifier: &str) -> String {
    format!(
        "{}{export_job_identifier}{EXPORT_OBJECT_SUFFIX}",
        prefix.unwrap_or_default()
    )
}

/// The results object's bytes: [`EXPORT_RESULT_COLUMNS`] as the header row, then one row
/// per item in `results`, oldest first — the order the search job recorded them in, which
/// is the order `ListSearchJobResults` pages them in.
///
/// `None` only when the CSV writer itself failed, which answers no object rather than a
/// truncated one: a consumer reading half a table would take it for the whole export.
/// An empty result set is NOT such a failure — it is a table with no rows, and the header
/// alone is what the export of an empty result set is.
#[must_use]
pub fn export_results_csv(results: &[ResultItem]) -> Option<Vec<u8>> {
    let mut writer = ::csv::Writer::from_writer(Vec::new());
    writer.write_record(EXPORT_RESULT_COLUMNS).ok()?;
    for item in results {
        writer.write_record(result_row(item)).ok()?;
    }
    writer.into_inner().ok()
}

/// One item's row, with every cell in the column [`EXPORT_RESULT_COLUMNS`] gives it.
///
/// The cells are named rather than positional, and the row is then laid out BY the column
/// list, so a row can neither drift out of step with the header nor carry a member under
/// another member's heading.
fn result_row(item: &ResultItem) -> Vec<String> {
    let cells: Vec<(&str, String)> = match item {
        ResultItem::S3ResultItem(item) => s3_cells(item),
        ResultItem::EBSResultItem(item) => ebs_cells(item),
    };
    EXPORT_RESULT_COLUMNS
        .iter()
        .map(|column| {
            cells
                .iter()
                .find(|(name, _)| name == column)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// The cells an Amazon S3 result item fills.
fn s3_cells(item: &S3ResultItem) -> Vec<(&'static str, String)> {
    vec![
        (ITEM_TYPE_COLUMN, S3_ITEM_TYPE.to_string()),
        ("BackupResourceArn", text(item.backup_resource_arn.as_ref())),
        ("SourceResourceArn", text(item.source_resource_arn.as_ref())),
        ("BackupVaultName", text(item.backup_vault_name.as_ref())),
        ("ObjectKey", text(item.object_key.as_ref())),
        ("ObjectSize", number(item.object_size)),
        ("ETag", text(item.e_tag.as_ref())),
        ("VersionId", text(item.version_id.as_ref())),
        ("CreationTime", instant(item.creation_time.as_ref())),
    ]
}

/// The cells an Amazon EBS result item fills.
fn ebs_cells(item: &EBSResultItem) -> Vec<(&'static str, String)> {
    vec![
        (ITEM_TYPE_COLUMN, EBS_ITEM_TYPE.to_string()),
        ("BackupResourceArn", text(item.backup_resource_arn.as_ref())),
        ("SourceResourceArn", text(item.source_resource_arn.as_ref())),
        ("BackupVaultName", text(item.backup_vault_name.as_ref())),
        (
            "FileSystemIdentifier",
            text(item.file_system_identifier.as_ref()),
        ),
        ("FilePath", text(item.file_path.as_ref())),
        ("FileSize", number(item.file_size)),
        ("CreationTime", instant(item.creation_time.as_ref())),
        (
            "LastModifiedTime",
            instant(item.last_modified_time.as_ref()),
        ),
    ]
}

/// A string member's cell: the value, or an empty cell for a member the item does not
/// carry.
fn text(value: Option<&String>) -> String {
    value.cloned().unwrap_or_default()
}

/// A numeric member's cell, rendered as the integer it is.
fn number(value: Option<i64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

/// A timestamp member's cell, rendered as the RFC 3339 instant the member names — the same
/// rendering a response carries it in. A member the item does not carry is an empty cell,
/// and so is an instant the renderer rejects: a cell carrying something that is not a
/// timestamp would be read as one. The latter is a renderer fault rather than a property of
/// the item, so it is traced (the instant itself is not logged).
fn instant(value: Option<&::aws_smithy_types::DateTime>) -> String {
    let Some(at) = value else {
        return String::new();
    };
    match at.fmt(::aws_smithy_types::date_time::Format::DateTime) {
        Ok(rendered) => rendered,
        Err(error) => {
            ::tracing::warn!(
                error = %error,
                "an exported result item's timestamp could not be rendered, so its cell is empty"
            );
            String::new()
        }
    }
}

/// Write `results` into the destination `specification` names, and answer what that came
/// to.
///
/// Called with NO state lock held: the write below is an await, and this service's store is
/// not held across one.
pub async fn deliver_export_results(
    callback: &EmbeddedEmulator,
    region: &str,
    specification: &ExportSpecification,
    export_job_identifier: &str,
    results: &[ResultItem],
) -> ExportDelivery {
    let ExportSpecification::s3ExportSpecification(destination) = specification;
    let key = export_object_key(
        destination.destination_prefix.as_deref(),
        export_job_identifier,
    );
    let Some(body) = export_results_csv(results) else {
        return ExportDelivery::Failed(format!(
            "The search results could not be written to s3://{}/{key}: the results could not be \
             framed as {EXPORT_MEDIA_TYPE}",
            destination.destination_bucket
        ));
    };
    // The emulator serves one object store over the in-process callback, so which Region it
    // is addressed in decides nothing — but the value ends up in an endpoint URL's
    // authority, so a scope that carries no Region label falls back to the default rather
    // than putting caller text there.
    let region = ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    let client = ::service_common::s3_client::s3_client(callback, region);
    let answered = client
        .put_object()
        .bucket(&destination.destination_bucket)
        .key(&key)
        .content_type(EXPORT_MEDIA_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    match answered {
        Ok(_) => ExportDelivery::Written(key),
        Err(error) => {
            // The caller's own values are rendered in their escaped `Debug` form, so a
            // bucket name or prefix carrying a newline cannot forge a log line; the result
            // items themselves are never logged.
            ::tracing::warn!(
                bucket = ?destination.destination_bucket,
                key = ?key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a search result export could not be written to the destination bucket"
            );
            ExportDelivery::Failed(format!(
                "The search results could not be written to s3://{}/{key}: {}",
                destination.destination_bucket,
                refusal_reason(&error)
            ))
        }
    }
}

/// Why the object store took no object, as IT reported it: the error code and message it
/// answered with, which is what a client is told the export failed on.
///
/// A failure that never reached the object store at all carries no such code — an emulator
/// assembled with no object store to write to — so the transport's own account of it is
/// reported instead, rather than a guess at which refusal it would have been.
fn refusal_reason<E, R>(error: &::aws_sdk_s3::error::SdkError<E, R>) -> String
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
    R: ::std::fmt::Debug,
    ::aws_sdk_s3::error::SdkError<E, R>: ::std::error::Error,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    match (error.code(), error.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => {
            ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the key is the destination prefix verbatim, then the export job's identifier
    /// and the format's suffix — so a listing under the prefix a client submitted finds it.
    #[test]
    fn the_results_object_key_starts_with_the_destination_prefix() {
        // The rows differ along what the rule generalises over: whether a prefix is
        // present at all, whether it ends in a separator, how many segments it has, and a
        // prefix that is a bare name rather than a path.
        let rows = [
            ("no prefix", None, "e1.csv"),
            ("an empty prefix", Some(""), "e1.csv"),
            ("a prefix ending in a separator", Some("results/"), "results/e1.csv"),
            ("a nested prefix", Some("a/b/c/"), "a/b/c/e1.csv"),
            ("a prefix with no separator", Some("exports-2026"), "exports-2026e1.csv"),
        ];
        for (case, prefix, expected) in rows {
            let actual = export_object_key(prefix, "e1");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the object is one CSV table — the modelled member names as its header, one row
    /// per recorded item, every cell under its own column — whatever kind of item the
    /// result set holds and however many.
    #[test]
    fn the_results_object_is_one_csv_table_over_the_modelled_members() {
        let s3_item = ResultItem::S3ResultItem(S3ResultItem {
            backup_resource_arn: Some("arn:aws:backup:us-west-2:000000000000:recovery-point:rp-1".to_string()),
            source_resource_arn: Some("arn:aws:s3:::bucket-1".to_string()),
            backup_vault_name: Some("vault".to_string()),
            // A value carrying the separator and a quote, so the framing is asserted and
            // not just the fields.
            object_key: Some("reports/\"q1\",final.txt".to_string()),
            object_size: Some(42),
            creation_time: Some(::aws_smithy_types::DateTime::from_secs(1_700_000_000)),
            e_tag: Some("\"abc\"".to_string()),
            version_id: None,
        });
        let ebs_item = ResultItem::EBSResultItem(EBSResultItem {
            backup_resource_arn: None,
            source_resource_arn: None,
            backup_vault_name: None,
            file_system_identifier: Some("fs-1".to_string()),
            file_path: Some("/var/log/messages".to_string()),
            file_size: Some(7),
            creation_time: None,
            last_modified_time: Some(::aws_smithy_types::DateTime::from_secs(1_700_000_001)),
        });
        let rows: [(&str, Vec<ResultItem>); 4] = [
            ("an empty result set", Vec::new()),
            ("one Amazon S3 item", vec![s3_item.clone()]),
            ("one Amazon EBS item", vec![ebs_item.clone()]),
            (
                "both item types, two of one",
                vec![s3_item.clone(), ebs_item.clone(), s3_item.clone()],
            ),
        ];
        for (case, results) in rows {
            let body = export_results_csv(&results)
                .unwrap_or_else(|| panic!("case {case:?}: the results must be framed"));
            let text = String::from_utf8(body)
                .unwrap_or_else(|error| panic!("case {case:?}: not UTF-8: {error}"));
            let mut reader = ::csv::Reader::from_reader(text.as_bytes());
            let header: Vec<String> = reader
                .headers()
                .unwrap_or_else(|error| panic!("case {case:?}: no header row: {error}"))
                .iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(
                header, EXPORT_RESULT_COLUMNS,
                "case {case:?}: expected the modelled member names as the header, got {header:?}"
            );
            let records: Vec<::csv::StringRecord> = reader
                .records()
                .collect::<Result<_, _>>()
                .unwrap_or_else(|error| panic!("case {case:?}: unreadable rows: {error}"));
            assert_eq!(
                records.len(),
                results.len(),
                "case {case:?}: expected one row per recorded item, got {records:?}"
            );
            for (index, (record, item)) in records.iter().zip(results.iter()).enumerate() {
                let expected_type = match item {
                    ResultItem::S3ResultItem(_) => S3_ITEM_TYPE,
                    ResultItem::EBSResultItem(_) => EBS_ITEM_TYPE,
                };
                assert_eq!(
                    record.get(0),
                    Some(expected_type),
                    "case {case:?} row {index}: expected the item's union member, got {record:?}"
                );
                assert_eq!(
                    record.len(),
                    EXPORT_RESULT_COLUMNS.len(),
                    "case {case:?} row {index}: expected a cell per column, got {record:?}"
                );
            }
        }
    }

    /// RULE: a value a cell carries is the member's own value, under its own column — the
    /// one that makes the quoting and the renderings assertable at the cell level.
    #[test]
    fn every_cell_carries_its_own_members_value() {
        let results = vec![ResultItem::S3ResultItem(S3ResultItem {
            backup_resource_arn: Some("arn:rp".to_string()),
            source_resource_arn: Some("arn:src".to_string()),
            backup_vault_name: Some("vault".to_string()),
            object_key: Some("a,b\"c\nd".to_string()),
            object_size: Some(1_024),
            creation_time: Some(::aws_smithy_types::DateTime::from_secs(1_700_000_000)),
            e_tag: Some("etag".to_string()),
            version_id: Some("v1".to_string()),
        })];
        let body = export_results_csv(&results).expect("the results must be framed");
        let text = String::from_utf8(body).expect("UTF-8");
        let mut reader = ::csv::Reader::from_reader(text.as_bytes());
        let record = reader
            .records()
            .next()
            .expect("one row")
            .expect("a readable row");
        let cell = |column: &str| -> String {
            let index = EXPORT_RESULT_COLUMNS
                .iter()
                .position(|name| *name == column)
                .unwrap_or_else(|| panic!("{column} is a column"));
            record.get(index).unwrap_or_default().to_string()
        };
        let expected = [
            (ITEM_TYPE_COLUMN, S3_ITEM_TYPE.to_string()),
            ("ObjectKey", "a,b\"c\nd".to_string()),
            ("ObjectSize", "1024".to_string()),
            ("ETag", "etag".to_string()),
            ("VersionId", "v1".to_string()),
            ("CreationTime", "2023-11-14T22:13:20Z".to_string()),
            // A member the Amazon S3 item does not declare is an empty cell.
            ("FilePath", String::new()),
            ("LastModifiedTime", String::new()),
        ];
        for (column, want) in expected {
            let actual = cell(column);
            assert_eq!(
                actual, want,
                "case {column:?}: expected {want:?}, got {actual:?}"
            );
        }
    }
}
