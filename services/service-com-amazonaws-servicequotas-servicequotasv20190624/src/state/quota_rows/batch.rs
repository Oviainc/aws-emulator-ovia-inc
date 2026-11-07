// SPDX-License-Identifier: CC0-1.0

//! AWS Batch's published default-quota catalog.

use super::*;

/// One AWS Batch API THROTTLE quota: the transactions per second the service accepts for
/// one operation of the account.
///
/// Batch publishes a row of this class for each throttled operation, and every member but
/// the operation's own name and the value is the same across the class — the `QuotaName` is
/// the operation's name in "Transactions per second for <Api> limit", the rate is published
/// as NOT adjustable, and the `Description` is the sentence Batch publishes for the class
/// with the operation named in it — so one constructor fills them and two rows of the class
/// cannot drift apart.
fn transactions_per_second(code: &str, api_name: &str, per_second: f64) -> QuotaRow {
    quota(
        format!("Transactions per second for {api_name} limit"),
        per_second,
    )
    .code(code)
    .fixed()
    .describe(&format!(
        "Maximum number of transactions per second (TPS) for each account for {api_name} \
         operations."
    ))
}

/// AWS Batch's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `batch` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code batch` reports, which is
/// the "AWS Batch service quotas" table of the AWS Batch User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota and for the
/// wording of the throttle rows. Every row carries the `L-` code AWS publishes the quota
/// under, so a client that read a code out of AWS's catalog resolves the same quota here.
///
/// Batch publishes its whole default set as NOT adjustable — the recorded listing reports
/// it so for the account's compute-environment and job-queue limits as much as for its
/// per-API throttles — so every row here is fixed.
pub(in crate::state) fn batch_quotas() -> Vec<QuotaRow> {
    vec![
        // The newer service-job plane's submission throttle, which AWS publishes without
        // the trailing "limit" the rest of the class carries.
        quota("Transactions per second for SubmitServiceJob", 5.0)
            .code("L-0194C9AB")
            .fixed()
            .describe(
                "Maximum number of transactions per second (TPS) for each account for \
                 SubmitServiceJob operations.",
            ),
        quota("Compute environment limit", 50.0)
            .code("L-144F0CA5")
            .fixed()
            .describe(
                "The maximum number of compute environments in this account in the current \
                 Region.",
            ),
        quota("Job queue limit", 50.0)
            .code("L-4CEA37AD")
            .fixed()
            .describe(
                "The maximum number of job queues in this account in the current Region.",
            ),
        quota("Array size limit", 10_000.0)
            .code("L-61E3E54E")
            .fixed()
            .describe("The maximum number of child jobs in one array job."),
        quota("Maximum number of jobs in SUBMITTED state", 1_000_000.0)
            .code("L-65F8BA2C")
            .fixed()
            .describe("The maximum number of jobs held in the SUBMITTED state at once."),
        quota("Scheduling policy limit", 100.0)
            .code("L-6B86CF1E")
            .fixed()
            .describe(
                "The maximum number of scheduling policies in this account in the current \
                 Region.",
            ),
        quota("Maximum number of share identifiers per scheduling policy", 500.0)
            .code("L-80D92D24")
            .fixed()
            .describe("The maximum number of share identifiers in one scheduling policy."),
        quota("Maximum number of compute environments per job queue", 3.0)
            .code("L-A2F30F35")
            .fixed()
            .describe("The maximum number of compute environments one job queue can use."),
        quota("Maximum number of job dependencies per job", 20.0)
            .code("L-AD075A76")
            .fixed()
            .describe("The maximum number of jobs one job can depend on."),
        quota("Consumable resource limit", 50.0)
            .code("L-B15D0E72")
            .fixed()
            .describe(
                "The maximum number of consumable resources in this account in the current \
                 Region.",
            ),
        transactions_per_second("L-C997A649", "RegisterJobDefinition", 50.0),
        quota("Job definition size limit", 24.0)
            .code("L-E6A2743D")
            .fixed()
            .describe("The maximum size, in KiB, of one job definition."),
        transactions_per_second("L-E985971C", "SubmitJob", 50.0),
        transactions_per_second("L-F8102809", "DescribeJobs", 50.0),
        quota("Service environment limit", 50.0)
            .code("L-F9381E33")
            .fixed()
            .describe(
                "The maximum number of service environments in this account in the current \
                 Region.",
            ),
    ]
}
