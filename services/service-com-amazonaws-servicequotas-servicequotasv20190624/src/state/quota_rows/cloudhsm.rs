// SPDX-License-Identifier: CC0-1.0

//! AWS CloudHSM's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name CloudHSM publishes its resource counts under.
const CLOUDHSM_USAGE_SERVICE: &str = "CloudHSM";

/// AWS CloudHSM's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cloudhsm` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code cloudhsm` reports, which
/// is the "AWS CloudHSM quotas" page of the AWS CloudHSM User Guide), and the recorded
/// default-quota listing of that code for which `QuotaCode` names which quota, for the
/// account-and-Region wording its counted planes carry, for each row's adjustability and
/// for the usage metric the cluster plane is metered by. CloudHSM counts a plane either per
/// Region and account or per CLUSTER — a cluster is the unit its HSMs, keys and users are
/// counted in — so a row names the plane it is counted on.
pub(in crate::state) fn cloudhsm_quotas() -> Vec<QuotaRow> {
    vec![
        quota("users per CloudHSM cluster", 1024.0)
            .code("L-0B35A0E6")
            .fixed()
            .describe("The maximum number of users in one CloudHSM cluster."),
        quota("HSMs per CloudHSM cluster", 28.0)
            .code("L-0D8B0541")
            .fixed()
            .describe("The maximum number of HSMs in one CloudHSM cluster."),
        quota("keys per hsm2m.medium CloudHSM cluster", 16_666.0)
            .code("L-194EB2B8")
            .fixed()
            .describe("The maximum number of keys in one cluster of hsm2m.medium HSMs."),
        quota("keys per CloudHSM cluster", 3300.0)
            .code("L-36814031")
            .describe("The maximum number of keys in one cluster of hsm1.medium HSMs."),
        resource_count_plane(
            "L-4B16B391",
            "Clusters per AWS Region and AWS account",
            4.0,
            CLOUDHSM_USAGE_SERVICE,
            "Clusters",
            "CloudHSM clusters",
        ),
        quota("backups per CloudHSM cluster", 20.0)
            .code("L-772318AE")
            .describe("The maximum number of backups retained for one CloudHSM cluster."),
        quota("HSMs per AWS Region and AWS account", 6.0)
            .code("L-95BA35D1")
            .describe("The maximum number of HSMs in this account in the current Region."),
        quota("client connections per HSM", 900.0)
            .code("L-F847CBBF")
            .fixed()
            .describe("The maximum number of client connections to one HSM."),
        quota("subnets per CloudHSM cluster", 8.0)
            .code("L-F9BBB650")
            .fixed()
            .describe("The maximum number of subnets one CloudHSM cluster can span."),
    ]
}
