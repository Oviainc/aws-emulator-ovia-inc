// SPDX-License-Identifier: CC0-1.0

//! Amazon Timestream's published default-quota catalog.

use super::*;

/// Amazon Timestream's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `timestream` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code timestream` reports,
/// which is the "Quotas" page of the Amazon Timestream Developer Guide), and the
/// `QuotaCode`s that catalog is keyed on.
///
/// Timestream publishes the planes of its data model — the databases, the tables, the
/// shape of one record and the retention of each store — beside the request rate of each
/// of its control-plane and query APIs. A rate row is built by [`request_rate`], so it
/// carries the one-second window AWS publishes it over and the `AWS/Usage` `CallCount` of
/// the API it throttles; a row that measures something carries the unit AWS publishes it
/// in rather than being a bare count.
pub(in crate::state) fn timestream_quotas() -> Vec<QuotaRow> {
    let rate = |code: &str, api: &str, per_second: f64| {
        request_rate(code, TIMESTREAM_USAGE_SERVICE, api, per_second)
    };
    vec![
        quota("Active queries per account", 1000.0)
            .code("L-00158752")
            .describe(
                "The maximum number of queries running at once in this account in the \
                 current Region.",
            ),
        rate("L-05CF2F29", "CancelQuery", 10.0),
        rate("L-0C6E4BCB", "CreateBatchLoadTask", 1.0),
        rate("L-1222731D", "CreateDatabase", 1.0),
        rate("L-16B72159", "CreateScheduledQuery", 1.0),
        rate("L-279D7B08", "CreateTable", 1.0),
        quota("Databases per account", 500.0)
            .code("L-3F031494")
            .describe(
                "The maximum number of databases in this account in the current Region.",
            ),
        rate("L-6044E01C", "DeleteDatabase", 1.0),
        rate("L-6B1915C6", "DeleteTable", 1.0),
        rate("L-6C25862A", "DescribeDatabase", 10.0),
        rate("L-71D80F70", "DescribeEndpoints", 10.0),
        rate("L-7565C2A8", "DescribeTable", 10.0),
        quota("Dimension name length", 60.0)
            .code("L-798CB19D")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one dimension name."),
        quota("Dimensions per series", 128.0)
            .code("L-7C9FE50E")
            .fixed()
            .describe("The maximum number of dimensions of one time series."),
        rate("L-8892F7F6", "ListDatabases", 10.0),
        rate("L-89E3B36D", "ListTables", 10.0),
        rate("L-8BF444C0", "ListTagsForResource", 10.0),
        quota("Magnetic store retention", 73_000.0)
            .code("L-8DFD99E7")
            .unit("Days")
            .describe(
                "The maximum time, in days, that a table retains data in its magnetic \
                 store.",
            ),
        quota("Measure name length", 256.0)
            .code("L-8FD9593E")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one measure name."),
        quota("Measure values per record", 256.0)
            .code("L-9F20DF3A")
            .fixed()
            .describe("The maximum number of measure values of one multi-measure record."),
        quota("Memory store retention", 8766.0)
            .code("L-A83B13DB")
            .unit("Hours")
            .describe(
                "The maximum time, in hours, that a table retains data in its memory store.",
            ),
        rate("L-AE604CE2", "Query", 10.0),
        quota("Query string length", 262_144.0)
            .code("L-BFCD0C9E")
            .unit("Bytes")
            .fixed()
            .describe("The maximum length, in bytes, of one query string."),
        quota("Query timeout", 60.0)
            .code("L-C0E9F2ED")
            .unit("Minutes")
            .fixed()
            .describe("The time, in minutes, after which a running query is cancelled."),
        quota("Records per WriteRecords request", 100.0)
            .code("L-C49D398A")
            .fixed()
            .describe("The maximum number of records in one WriteRecords request."),
        rate("L-F07A9AB4", "WriteRecords", 1000.0),
        quota("Scheduled queries per account", 10_000.0)
            .code("L-F1AC16A2")
            .describe(
                "The maximum number of scheduled queries in this account in the current \
                 Region.",
            ),
        quota("Tables per database", 50_000.0)
            .code("L-F98EACE7")
            .describe("The maximum number of tables in one database."),
        quota("Unique measure names per table", 1024.0)
            .code("L-FA296006")
            .fixed()
            .describe("The maximum number of distinct measure names in one table."),
        rate("L-FD5A0C1A", "UpdateTable", 1.0),
        quota("Batch load tasks per account", 100.0)
            .code("L-FEB34FC6")
            .describe(
                "The maximum number of batch load tasks in this account in the current \
                 Region.",
            ),
    ]
}

/// The name `AWS/Usage` publishes Amazon Timestream's API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const TIMESTREAM_USAGE_SERVICE: &str = "Timestream";
