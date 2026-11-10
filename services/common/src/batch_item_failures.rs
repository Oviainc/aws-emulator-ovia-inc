// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the PARTIAL BATCH RESPONSE a consumer of a batch gets back from
//! whatever it handed that batch to: which records of the batch the attempt reports it did
//! not process.
//!
//! Every service that polls a source for a batch and hands it over in one call reads the
//! same document, because the report IS the same document whatever the source and whatever
//! the attempt: a `batchItemFailures` list whose entries each name one record of the batch
//! by its `itemIdentifier`. Only the identifier of a record differs with the source — a
//! queue record is named by its message id and a stream record by its sequence number — so
//! the caller passes the identifiers of the batch it handed over, in the order it handed
//! them over, and gets back which of them the attempt left behind. What that then means is
//! the SOURCE's own: a queue's processed messages are deleted and the rest stay on the queue
//! to reappear when their visibility timeout lapses, and a stream's read position moves
//! only past the records that were processed.
//!
//! The reading is the one AWS publishes for the report:
//!
//! * The batch was WHOLLY PROCESSED when the response names no failed record: no response at
//!   all, a null response, a response carrying no `batchItemFailures`, and a
//!   `batchItemFailures` that is null or empty.
//! * The batch WHOLLY FAILED — every record of it is retried — when the response cannot be
//!   read as a report of records of that batch: a response that is not JSON, a
//!   `batchItemFailures` that is not a list, an entry that carries no `itemIdentifier` key,
//!   an `itemIdentifier` that is not a non-empty string, and an `itemIdentifier` naming a
//!   record the batch does not hold. A report that names EVERY record of the batch is the
//!   same answer by a different route: it leaves nothing of the batch processed.
//! * Otherwise exactly the records the report names were not processed, and every other
//!   record of the batch was.

use std::collections::BTreeSet;

use ::serde_json::Value;

/// The member of the attempt's response document that carries the report.
const BATCH_ITEM_FAILURES: &str = "batchItemFailures";

/// The member of one entry of that list that names a record of the batch.
const ITEM_IDENTIFIER: &str = "itemIdentifier";

/// What one attempt's response says about the records of the batch it was handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchReport {
    /// Every record of the batch was processed.
    WholeBatchProcessed,
    /// No record of the batch was processed, and why the response says so.
    WholeBatchFailed(String),
    /// The records the report named, in the order the batch holds them. These were not
    /// processed; every other record of the batch was. Never empty, and never every record
    /// of the batch — a report that names them all is [`Self::WholeBatchFailed`], because it
    /// leaves nothing processed.
    RecordsFailed(Vec<String>),
}

/// Read `payload` — the response of the attempt that was handed the batch — as the report of
/// which of `identifiers` it did not process.
///
/// `identifiers` are the records of the batch, in the order they were handed over. See this
/// module's documentation for the three answers and what each is read from.
#[must_use]
pub fn read_batch_item_failures(payload: Option<&[u8]>, identifiers: &[String]) -> BatchReport {
    let Some(bytes) = payload.filter(|bytes| !bytes.is_empty()) else {
        // No response document at all names no failed record, which is the batch processed.
        return BatchReport::WholeBatchProcessed;
    };
    let Ok(document) = ::serde_json::from_slice::<Value>(bytes) else {
        return BatchReport::WholeBatchFailed(
            "the response is not a JSON document, so it reports no record of the batch as \
             processed"
                .to_string(),
        );
    };
    // A response that names no `batchItemFailures` — including one that is not a document
    // with members at all — reports no failed record.
    let Some(reported) = document.get(BATCH_ITEM_FAILURES) else {
        return BatchReport::WholeBatchProcessed;
    };
    if reported.is_null() {
        return BatchReport::WholeBatchProcessed;
    }
    let Some(entries) = reported.as_array() else {
        return BatchReport::WholeBatchFailed(format!(
            "the response carries a {BATCH_ITEM_FAILURES} that is not a list of the records \
             that failed"
        ));
    };
    // Both directions of the membership question below are asked through a SET, so the work
    // one report costs is its own length and never the product of the report's length and
    // the batch's: an attempt answering a long list cannot make the reading expensive.
    let held: BTreeSet<&str> = identifiers.iter().map(String::as_str).collect();
    let mut named: BTreeSet<&str> = BTreeSet::new();
    for entry in entries {
        let Some(identifier) = entry.get(ITEM_IDENTIFIER) else {
            return BatchReport::WholeBatchFailed(format!(
                "an entry of the {BATCH_ITEM_FAILURES} carries no {ITEM_IDENTIFIER}"
            ));
        };
        let Some(identifier) = identifier.as_str().filter(|text| !text.is_empty()) else {
            return BatchReport::WholeBatchFailed(format!(
                "an entry of the {BATCH_ITEM_FAILURES} carries an {ITEM_IDENTIFIER} that is \
                 not the identifier of a record"
            ));
        };
        if !held.contains(identifier) {
            return BatchReport::WholeBatchFailed(format!(
                "the {BATCH_ITEM_FAILURES} names {identifier}, which is not a record of the \
                 batch that was handed over"
            ));
        }
        named.insert(identifier);
    }
    // In the order the batch holds them, and each one once however often it was named: the
    // answer is a set of records of this batch, not a transcription of the report.
    let failed: Vec<String> = identifiers
        .iter()
        .filter(|identifier| named.contains(identifier.as_str()))
        .cloned()
        .collect();
    if failed.is_empty() {
        return BatchReport::WholeBatchProcessed;
    }
    if failed.len() == identifiers.len() {
        return BatchReport::WholeBatchFailed(format!(
            "the {BATCH_ITEM_FAILURES} names every record of the batch that was handed over, \
             so none of them was processed"
        ));
    }
    BatchReport::RecordsFailed(failed)
}

/// The records of `identifiers` the attempt PROCESSED, given what its response reported.
///
/// `None` when no record of the batch was processed, which is the answer for a report that
/// could not be read — the whole batch is retried, so nothing of it is consumed.
#[must_use]
pub fn processed_records(report: &BatchReport, identifiers: &[String]) -> Option<Vec<String>> {
    match report {
        BatchReport::WholeBatchProcessed => Some(identifiers.to_vec()),
        BatchReport::WholeBatchFailed(_) => None,
        BatchReport::RecordsFailed(failed) => {
            let left_behind: BTreeSet<&str> = failed.iter().map(String::as_str).collect();
            Some(
                identifiers
                    .iter()
                    .filter(|identifier| !left_behind.contains(identifier.as_str()))
                    .cloned()
                    .collect(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use ::serde_json::json;

    use super::{processed_records, read_batch_item_failures, BatchReport};

    /// The identifiers of the batch every row below reports about.
    fn batch() -> Vec<String> {
        vec!["r-1".to_string(), "r-2".to_string(), "r-3".to_string()]
    }

    #[test]
    fn a_response_naming_no_failed_record_is_the_whole_batch_processed() {
        // Each row is one of the ways AWS publishes for a report that names no failed
        // record: an absent response, a null one, one carrying no `batchItemFailures`, and a
        // `batchItemFailures` that is null or empty. The last two rows are documents an
        // attempt that knows nothing of the report returns.
        let rows: [(&str, Option<Vec<u8>>); 7] = [
            ("no payload at all", None),
            ("an empty payload", Some(Vec::new())),
            ("a null response", Some(b"null".to_vec())),
            (
                "a response carrying no batchItemFailures",
                Some(json!({ "statusCode": 200 }).to_string().into_bytes()),
            ),
            (
                "a null batchItemFailures",
                Some(
                    json!({ "batchItemFailures": Option::<i32>::None })
                        .to_string()
                        .into_bytes(),
                ),
            ),
            (
                "an empty batchItemFailures",
                Some(json!({ "batchItemFailures": [] }).to_string().into_bytes()),
            ),
            (
                "a response that is not a document with members",
                Some(json!(["ok"]).to_string().into_bytes()),
            ),
        ];
        for (case, payload) in rows {
            let report = read_batch_item_failures(payload.as_deref(), &batch());
            assert_eq!(
                report,
                BatchReport::WholeBatchProcessed,
                "case {case:?}: the batch must be wholly processed, got {report:?}"
            );
            assert_eq!(
                processed_records(&report, &batch()),
                Some(batch()),
                "case {case:?}: every record of the batch must count as processed"
            );
        }
    }

    #[test]
    fn a_report_that_cannot_be_read_fails_the_whole_batch() {
        // The catalog of conditions AWS publishes as a complete batch failure: an invalid
        // JSON response, an `itemIdentifier` that is an empty string or null, an entry whose
        // key is not `itemIdentifier`, and an `itemIdentifier` naming no record of the batch.
        // A `batchItemFailures` that is not a list at all is the same unreadable report.
        let rows: [(&str, Vec<u8>); 8] = [
            ("a response that is not JSON", b"not json at all".to_vec()),
            (
                "a batchItemFailures that is not a list",
                json!({ "batchItemFailures": { "itemIdentifier": "r-1" } })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "an empty itemIdentifier",
                json!({ "batchItemFailures": [{ "itemIdentifier": "" }] })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "a null itemIdentifier",
                json!({ "batchItemFailures": [{ "itemIdentifier": Option::<i32>::None }] })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "an itemIdentifier that is not a string",
                json!({ "batchItemFailures": [{ "itemIdentifier": 1 }] })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "a bad key name",
                json!({ "batchItemFailures": [{ "ItemIdentifier": "r-1" }] })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "an identifier naming no record of the batch",
                json!({ "batchItemFailures": [{ "itemIdentifier": "r-1" }, { "itemIdentifier": "r-9" }] })
                    .to_string()
                    .into_bytes(),
            ),
            (
                "every record of the batch, which leaves nothing processed",
                json!({ "batchItemFailures": [
                    { "itemIdentifier": "r-1" },
                    { "itemIdentifier": "r-2" },
                    { "itemIdentifier": "r-3" },
                ] })
                .to_string()
                .into_bytes(),
            ),
        ];
        for (case, payload) in rows {
            let report = read_batch_item_failures(Some(&payload), &batch());
            assert!(
                matches!(report, BatchReport::WholeBatchFailed(_)),
                "case {case:?}: the whole batch must be reported failed, got {report:?}"
            );
            assert_eq!(
                processed_records(&report, &batch()),
                None,
                "case {case:?}: no record of the batch may count as processed"
            );
        }
    }

    #[test]
    fn the_records_the_report_names_are_the_ones_the_attempt_left_behind() {
        // The rows differ along how many of the batch the report names, where in the batch
        // they are, the order the report names them in, and whether it names one twice.
        // One row: the case, the response the attempt answered, the records it must leave
        // behind, and the records the attempt must then consume.
        type Row = (&'static str, Vec<u8>, Vec<&'static str>, Vec<&'static str>);
        let rows: [Row; 5] = [
            (
                "the first record",
                json!({ "batchItemFailures": [{ "itemIdentifier": "r-1" }] })
                    .to_string()
                    .into_bytes(),
                vec!["r-1"],
                vec!["r-2", "r-3"],
            ),
            (
                "the last record",
                json!({ "batchItemFailures": [{ "itemIdentifier": "r-3" }] })
                    .to_string()
                    .into_bytes(),
                vec!["r-3"],
                vec!["r-1", "r-2"],
            ),
            (
                "two records, reported out of batch order",
                json!({ "batchItemFailures": [{ "itemIdentifier": "r-3" }, { "itemIdentifier": "r-1" }] })
                    .to_string()
                    .into_bytes(),
                vec!["r-1", "r-3"],
                vec!["r-2"],
            ),
            (
                "one record named twice",
                json!({ "batchItemFailures": [{ "itemIdentifier": "r-2" }, { "itemIdentifier": "r-2" }] })
                    .to_string()
                    .into_bytes(),
                vec!["r-2"],
                vec!["r-1", "r-3"],
            ),
            (
                "all but the last record",
                json!({ "batchItemFailures": [{ "itemIdentifier": "r-1" }, { "itemIdentifier": "r-2" }] })
                    .to_string()
                    .into_bytes(),
                vec!["r-1", "r-2"],
                vec!["r-3"],
            ),
        ];
        for (case, payload, failed, processed) in rows {
            let report = read_batch_item_failures(Some(&payload), &batch());
            let expected: Vec<String> = failed.iter().map(|id| (*id).to_string()).collect();
            assert_eq!(
                report,
                BatchReport::RecordsFailed(expected.clone()),
                "case {case:?}: expected the records {expected:?} to be the ones left behind, \
                 got {report:?}"
            );
            let expected_processed: Vec<String> =
                processed.iter().map(|id| (*id).to_string()).collect();
            assert_eq!(
                processed_records(&report, &batch()),
                Some(expected_processed.clone()),
                "case {case:?}: expected the records {expected_processed:?} to count as \
                 processed"
            );
        }
    }
}
