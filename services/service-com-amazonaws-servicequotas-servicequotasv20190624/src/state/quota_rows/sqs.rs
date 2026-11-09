// SPDX-License-Identifier: CC0-1.0

//! Amazon Simple Queue Service's published default-quota catalog.

use super::*;

/// Amazon SQS's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `sqs` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code sqs` reports, which is
/// the "Amazon SQS quotas" page of the Amazon SQS Developer Guide), and the `QuotaCode`s
/// that catalog is keyed on.
///
/// SQS publishes almost all of its quotas as NOT adjustable: they are properties of the
/// queue the service implements — how large a message may be, how long it may be hidden
/// or kept, how many of them may be in flight — rather than allowances an account may ask
/// to have raised. A row that measures something carries the unit AWS publishes it in
/// (bytes for a size, seconds for a window, days for a retention) rather than being a
/// bare count.
pub(in crate::state) fn sqs_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Batch Entries per Batch Request", 10.0)
            .code("L-2DA3E3B2")
            .fixed()
            .describe("The number of entries in a batch request."),
        quota("Delay Queue Delay", 900.0)
            .code("L-49DA5CEC")
            .unit("Seconds")
            .fixed()
            .describe("The delay, in seconds, that a delay queue applies to a message."),
        quota("Maximum Receives per Redrive Policy", 1000.0)
            .code("L-4BE1B2BD")
            .fixed()
            .describe(
                "The number of times a message is received before a redrive policy moves \
                 it to the dead-letter queue.",
            ),
        quota("In Flight Messages per FIFO Queue", 20_000.0)
            .code("L-4FF3DAAF")
            .fixed()
            .describe("The number of in flight messages for a FIFO queue."),
        quota("Message Attributes per Message", 10.0)
            .code("L-6A03DCE9")
            .fixed()
            .describe("The number of message attributes in a message."),
        quota("Message Group ID Length", 128.0)
            .code("L-79B6240C")
            .fixed()
            .describe("The number of characters in a message group ID."),
        quota("Message Retention Period", 14.0)
            .code("L-81970B52")
            .unit("Days")
            .fixed()
            .describe("The time, in days, that a queue retains a message."),
        quota("Message Size", 262_144.0)
            .code("L-98134C75")
            .unit("Bytes")
            .fixed()
            .describe("The size, in bytes, of a message."),
        quota("In Flight Messages per Standard Queue", 120_000.0)
            .code("L-9F628B95")
            .fixed()
            .describe("The number of in flight messages for a standard queue."),
        quota("Queue Name Length", 80.0)
            .code("L-A01B4DF0")
            .fixed()
            .describe("The number of characters in a queue name."),
        quota("Queue Policy Size", 8192.0)
            .code("L-A7816957")
            .unit("Bytes")
            .fixed()
            .describe("The size, in bytes, of a queue policy."),
        quota("Tags per Queue", 50.0)
            .code("L-B2A3B9D5")
            .fixed()
            .describe("The number of tags on a queue."),
        // The two throughput planes of the service: the per-API transaction rate of a
        // standard queue and the one a FIFO queue serves. Both are published as a RATE,
        // so each carries the one-second window AWS publishes it over.
        quota("Transactions per Second per API Action", 3000.0)
            .code("L-BBEFA6CF")
            .period(1, "SECOND")
            .describe("The number of transactions per second for each API action."),
        quota("FIFO Queue Transactions per Second", 3000.0)
            .code("L-BF2A6161")
            .period(1, "SECOND")
            .fixed()
            .describe(
                "The number of transactions per second that a FIFO queue serves for each \
                 API action.",
            ),
        quota("Long Polling Wait Time", 20.0)
            .code("L-C491D5A4")
            .unit("Seconds")
            .fixed()
            .describe("The time, in seconds, that a receive request waits for a message."),
        quota("Visibility Timeout", 43_200.0)
            .code("L-E8A9C91E")
            .unit("Seconds")
            .fixed()
            .describe(
                "The time, in seconds, that a received message is hidden from the other \
                 consumers of the queue.",
            ),
        quota("Message Deduplication ID Length", 128.0)
            .code("L-F115E65A")
            .fixed()
            .describe("The number of characters in a message deduplication ID."),
        // The principal plane of a queue policy: a count of the principals the policy
        // document may name, which AWS publishes as fixed.
        quota("Principals per Queue Policy", 50.0)
            .code("L-F61F33C3")
            .fixed()
            .describe("The number of principals in a queue policy."),
    ]
}
