// SPDX-License-Identifier: CC0-1.0

//! Whether an event source mapping READS the partial batch response of the function it
//! hands a batch to, and the reading of that report.
//!
//! `FunctionResponseTypes` is "(Kinesis, DynamoDB Streams, Amazon MSK, self-managed Apache
//! Kafka, and Amazon SQS) A list of current response type enums applied to the event source
//! mapping", and the one value `FunctionResponseType` has is
//! [`REPORT_BATCH_ITEM_FAILURES`]. That setting is what makes a mapping READ the invocation's
//! response for the records it names, instead of judging the whole batch by nothing but
//! whether the invocation succeeded. A mapping that does not carry it reads no report at
//! all — so a `batchItemFailures` such a mapping's function returns changes nothing — which
//! is the whole difference between the two. That question is this service's own, because
//! `FunctionResponseTypes` is a member of ITS API.
//!
//! The reading of the report itself is not: the document is the same whatever polls a source
//! in batches and hands one over in a single call, so it lives in
//! [`service_common::batch_item_failures`] and is re-exported here — every source a mapping
//! retries records of is served by that one reading, which answers which of the identifiers
//! the caller handed over the attempt left behind. What that then means is the source's own
//! ([`super::sqs_poller`] deletes the processed messages and leaves the rest on the queue;
//! [`super::event_source_poller`] checkpoints a shard past the records it processed).

pub use ::service_common::batch_item_failures::{
    processed_records, read_batch_item_failures, BatchReport,
};

/// The one value `FunctionResponseType` has, from
/// `smithy_query(query="shape", shape="FunctionResponseType")`: the response type that makes
/// a mapping read its function's report of which records of a batch failed. It is the one
/// member of the generated `crate::ops::types::shared_types::FUNCTION_RESPONSE_TYPE_VALUES`,
/// which a test below holds this constant to.
pub const REPORT_BATCH_ITEM_FAILURES: &str = "ReportBatchItemFailures";

/// Whether `function_response_types` — the mapping's `FunctionResponseTypes`, as stored —
/// asks for the report to be read.
#[must_use]
pub fn reports_batch_item_failures(function_response_types: &[String]) -> bool {
    function_response_types
        .iter()
        .any(|response_type| response_type == REPORT_BATCH_ITEM_FAILURES)
}

#[cfg(test)]
mod tests {
    use super::{reports_batch_item_failures, REPORT_BATCH_ITEM_FAILURES};

    #[test]
    fn the_response_type_read_here_is_the_one_the_model_declares() {
        // The generated list of what `FunctionResponseType` admits, so this reading and the
        // value the wire accepts cannot drift apart.
        let declared = crate::ops::types::shared_types::FUNCTION_RESPONSE_TYPE_VALUES;
        assert!(
            declared.contains(&REPORT_BATCH_ITEM_FAILURES),
            "the response type this module reads a report for must be one the model declares, \
             got {declared:?}"
        );
    }

    #[test]
    fn only_the_declared_response_type_asks_for_the_report_to_be_read() {
        // The rows differ along what the mapping's `FunctionResponseTypes` holds: nothing,
        // the one value the enum has, and a value that is not that one.
        let rows: [(&str, Vec<String>, bool); 3] = [
            ("no response type", Vec::new(), false),
            (
                "the one value the enum has",
                vec![REPORT_BATCH_ITEM_FAILURES.to_string()],
                true,
            ),
            (
                "some other value",
                vec!["ReportSomethingElse".to_string()],
                false,
            ),
        ];
        for (case, types, expected) in rows {
            let reads = reports_batch_item_failures(&types);
            assert_eq!(
                reads, expected,
                "case {case:?}: expected reports_batch_item_failures to be {expected}, got \
                 {reads}"
            );
        }
    }
}
