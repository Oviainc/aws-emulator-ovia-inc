// SPDX-License-Identifier: CC0-1.0

//! AWS Cloud9's published default-quota catalog.

use super::*;

/// AWS Cloud9's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cloud9` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code cloud9` reports, which is
/// the "AWS Cloud9 service quotas" table of the AWS Cloud9 User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota. Cloud9
/// counts its development environments on TWO planes — the account's and each user's — and
/// publishes a quota per environment KIND on each, so a row names the plane and the kind it
/// counts.
pub(in crate::state) fn cloud9_quotas() -> Vec<QuotaRow> {
    vec![
        quota("SSH development environments for this user", 100.0)
            .code("L-5D26553C")
            .describe(
                "The maximum number of SSH development environments one user can own in this \
                 account in the current Region.",
            ),
        quota("Shared environment members", 8.0)
            .code("L-65F36924")
            .fixed()
            .describe(
                "The maximum number of members one development environment can be shared with.",
            ),
        quota("EC2 development environments for this account", 200.0)
            .code("L-8ECD6BDB")
            .describe(
                "The maximum number of EC2 development environments in this account in the \
                 current Region.",
            ),
        quota("SSH development environments for this account", 200.0)
            .code("L-A29EDECE")
            .describe(
                "The maximum number of SSH development environments in this account in the \
                 current Region.",
            ),
        quota("EC2 development environments for this user", 100.0)
            .code("L-C1302C17")
            .describe(
                "The maximum number of EC2 development environments one user can own in this \
                 account in the current Region.",
            ),
    ]
}
