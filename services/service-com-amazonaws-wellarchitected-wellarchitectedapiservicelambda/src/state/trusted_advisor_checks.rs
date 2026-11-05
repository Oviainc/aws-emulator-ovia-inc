// SPDX-License-Identifier: CC0-1.0

//! The Trusted Advisor check content this emulator publishes, and the one lookup every
//! operation that reports a check goes through.
//!
//! A workload's Trusted Advisor checks are "Trusted Advisor check details by account
//! related to the workload"
//! (`smithy_query(query="operation", operation="ListCheckDetails")`): a check belongs to
//! ONE best practice of ONE question of a lens — `CheckDetail` carries the `LensArn`,
//! `PillarId`, `QuestionId` and `ChoiceId` it is associated to
//! (`smithy_query(query="shape", shape="CheckDetail")`) — and it is reported once per
//! account the workload covers. Only SOME best practices have checks, so the table below
//! is the association, and a best practice with no row here has no check to report.
//!
//! ## Where the content comes from
//!
//! Each row's [`TrustedAdvisorCheck::name`] is a check Amazon Web Services publishes in
//! its Trusted Advisor check reference
//! (<https://docs.aws.amazon.com/awssupport/latest/user/trusted-advisor-check-reference.html>),
//! and [`TrustedAdvisorCheck::best_practice`] is the best practice of the AWS
//! Well-Architected Framework lens it inspects — named by its `Choice$Title` in
//! [`super::FRAMEWORK_LENS_QUESTIONS`], which is where this emulator holds that lens's
//! content. The `ChoiceId` a check is reported under is therefore never written here: it
//! is resolved through the question's own choice catalog
//! ([`super::ReviewQuestion::choices`]), so the id a check carries is the id an answer of
//! that question is recorded under, whatever the catalog's ordering.
//!
//! [`TrustedAdvisorCheck::id`] and [`TrustedAdvisorCheck::description`] are this
//! emulator's own: a check's wire id is an opaque value neither the Smithy model nor the
//! service docs publish, so each id here is a stable slug of the check's name rather than
//! a guess at AWS's. A test pins that every row resolves to a choice of the question it
//! names and that the ids are unique.
//!
//! ## What a check can SAY here
//!
//! The association above is content; a check's RESULT is not. A result is what Trusted
//! Advisor read from the resources of one account, which this emulator neither runs nor
//! holds — so the only thing it can report for a check is WHY there is no result for that
//! account, which is exactly what `CheckStatus`'s two non-result members and
//! `CheckFailureReason` are for ([`check_result`]). `OKAY`, `WARNING` and `ERROR` state a
//! finding about a caller's resources, so this emulator never reports one, and
//! `FlaggedResources` (`@range(min: 1)`) counts resources a check flagged, so a check that
//! produced no result carries none.

use super::{lens_alias, ReviewQuestion, FRAMEWORK_LENS_ALIAS};

/// `CheckProvider$TRUSTED_ADVISOR` — "provider of the check related to the best practice",
/// whose enum publishes exactly that one value
/// (`smithy_query(query="shape", shape="CheckProvider")`), so it is what every check this
/// emulator reports is provided by.
pub const CHECK_PROVIDER_TRUSTED_ADVISOR: &str = "TRUSTED_ADVISOR";

/// `CheckStatus$NOT_AVAILABLE`: the check has no result for the account because the
/// account cannot produce one (`smithy_query(query="shape", shape="CheckStatus")`).
pub const CHECK_STATUS_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// `CheckStatus$FETCH_FAILED`: the check's result could not be fetched for the account at
/// all (`smithy_query(query="shape", shape="CheckStatus")`).
pub const CHECK_STATUS_FETCH_FAILED: &str = "FETCH_FAILED";

/// `CheckFailureReason$PREMIUM_SUPPORT_REQUIRED`: the account's support plan does not
/// provide the Trusted Advisor checks
/// (`smithy_query(query="shape", shape="CheckFailureReason")`).
pub const CHECK_REASON_PREMIUM_SUPPORT_REQUIRED: &str = "PREMIUM_SUPPORT_REQUIRED";

/// `CheckFailureReason$ASSUME_ROLE_ERROR`: the role that reads Trusted Advisor in the
/// account could not be assumed
/// (`smithy_query(query="shape", shape="CheckFailureReason")`).
pub const CHECK_REASON_ASSUME_ROLE_ERROR: &str = "ASSUME_ROLE_ERROR";

/// One Trusted Advisor check this emulator publishes, and the best practice it inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedAdvisorCheck {
    /// `CheckDetail$Id` / `CheckSummary$Id` — "Trusted Advisor check ID". This emulator's
    /// own stable slug of [`Self::name`]; see the module docs.
    pub id: &'static str,
    /// `CheckDetail$Name` — "Trusted Advisor check name", as AWS publishes it in the
    /// Trusted Advisor check reference.
    pub name: &'static str,
    /// `CheckDetail$Description` — "Trusted Advisor check description": what the check
    /// inspects, in one sentence.
    pub description: &'static str,
    /// The `QuestionId` of the framework-lens question whose best practice this check
    /// inspects ([`super::FRAMEWORK_LENS_QUESTIONS`]).
    pub question_id: &'static str,
    /// The `Choice$Title` of the best practice, within that question's choice catalog.
    /// The `ChoiceId` is resolved from it rather than restated, so the two cannot drift.
    pub best_practice: &'static str,
}

/// Every check this emulator publishes, grouped by the question whose best practices they
/// inspect and in the framework lens's own pillar order — which is the order the checks of
/// one best practice are reported in.
pub const TRUSTED_ADVISOR_CHECK_CATALOG: &[TrustedAdvisorCheck] = &[
    // Security.
    check(
        "mfa-on-root-account",
        "MFA on Root Account",
        "Checks whether multi-factor authentication is enabled for the root user of the \
         account.",
        "securely-operate",
        "Secure account root user and properties",
    ),
    check(
        "exposed-access-keys",
        "Exposed Access Keys",
        "Checks for access keys of the account that have been exposed publicly and for \
         irregular usage of them.",
        "identities",
        "Store and use secrets securely",
    ),
    check(
        "iam-access-key-rotation",
        "IAM Access Key Rotation",
        "Checks for active IAM access keys that have not been rotated recently.",
        "identities",
        "Audit and rotate credentials periodically",
    ),
    check(
        "amazon-s3-bucket-permissions",
        "Amazon S3 Bucket Permissions",
        "Checks buckets for open access permissions or access granted to any \
         authenticated AWS user.",
        "permissions",
        "Analyze public and cross-account access",
    ),
    check(
        "amazon-rds-public-snapshots",
        "Amazon RDS Public Snapshots",
        "Checks the permission settings of database snapshots for snapshots that are \
         marked public.",
        "permissions",
        "Analyze public and cross-account access",
    ),
    check(
        "amazon-ebs-public-snapshots",
        "Amazon EBS Public Snapshots",
        "Checks the permission settings of volume snapshots for snapshots that are marked \
         public.",
        "permissions",
        "Analyze public and cross-account access",
    ),
    check(
        "aws-cloudtrail-logging",
        "AWS CloudTrail Logging",
        "Checks for trails that are not logging, and for trails whose log delivery to \
         Amazon S3 is failing.",
        "detect-investigate-events",
        "Configure service and application logging",
    ),
    check(
        "security-groups-specific-ports-unrestricted",
        "Security Groups - Specific Ports Unrestricted",
        "Checks security groups for rules that allow unrestricted access to specific \
         ports.",
        "network-protection",
        "Control traffic flow within your network layers",
    ),
    check(
        "amazon-rds-security-group-access-risk",
        "Amazon RDS Security Group Access Risk",
        "Checks database security group rules for rules that grant overly permissive \
         access.",
        "network-protection",
        "Control traffic flow within your network layers",
    ),
    check(
        "elb-listener-security",
        "ELB Listener Security",
        "Checks load balancer listeners for the absence of a secure protocol and for \
         outdated cipher suites.",
        "protect-data-in-transit",
        "Enforce encryption in transit",
    ),
    // Reliability.
    check(
        "service-limits",
        "Service Limits",
        "Checks usage of the account against the service quotas that apply to it.",
        "manage-service-quotas",
        "Monitor and manage quotas",
    ),
    check(
        "amazon-ebs-snapshots",
        "Amazon EBS Snapshots",
        "Checks volumes for a missing snapshot and for snapshots that are older than the \
         recovery point they are relied on for.",
        "back-up-data",
        "Identify and back up all data that needs to be backed up, or reproduce the data \
         from sources",
    ),
    check(
        "amazon-rds-backups",
        "Amazon RDS Backups",
        "Checks database instances for automated backups that are disabled or kept for \
         too short a retention period.",
        "back-up-data",
        "Perform data backup automatically",
    ),
    check(
        "amazon-rds-multi-az",
        "Amazon RDS Multi-AZ",
        "Checks database instances that are deployed in a single Availability Zone.",
        "fault-isolation",
        "Deploy the workload to multiple locations",
    ),
    check(
        "elb-cross-zone-load-balancing",
        "ELB Cross-Zone Load Balancing",
        "Checks load balancers for cross-zone load balancing that is not enabled.",
        "fault-isolation",
        "Deploy the workload to multiple locations",
    ),
    check(
        "auto-scaling-group-health-check",
        "Auto Scaling Group Health Check",
        "Checks Auto Scaling groups whose associated load balancer health check is not \
         the configured health check type.",
        "withstand-component-failures",
        "Fail over to healthy resources",
    ),
    // Performance efficiency.
    check(
        "high-utilization-amazon-ec2-instances",
        "High Utilization Amazon EC2 Instances",
        "Checks instances that ran at consistently high CPU utilization, which can \
         constrain the workload.",
        "compute-architecture-selection",
        "Configure and right-size compute resources",
    ),
    check(
        "cloudfront-content-delivery-optimization",
        "CloudFront Content Delivery Optimization",
        "Checks for data transfer out of Amazon S3 buckets that would be served faster \
         through a content delivery network.",
        "data-management",
        "Implement data access patterns that utilize caching",
    ),
    // Cost optimization.
    check(
        "idle-load-balancers",
        "Idle Load Balancers",
        "Checks load balancers whose request count makes them idle, so they are paid for \
         and not used.",
        "decomission-resources",
        "Decommission resources",
    ),
    check(
        "unassociated-elastic-ip-addresses",
        "Unassociated Elastic IP Addresses",
        "Checks for allocated Elastic IP addresses that are not associated with a running \
         instance.",
        "decomission-resources",
        "Decommission resources",
    ),
    check(
        "amazon-rds-idle-db-instances",
        "Amazon RDS Idle DB Instances",
        "Checks database instances that have had no connection for a prolonged period.",
        "decomission-resources",
        "Decommission resources",
    ),
    check(
        "low-utilization-amazon-ec2-instances",
        "Low Utilization Amazon EC2 Instances",
        "Checks instances that ran at consistently low CPU utilization and network \
         traffic, so they are over-provisioned.",
        "resource-type-size-number",
        "Select resource type, size, and number based on data",
    ),
    check(
        "amazon-ec2-reserved-instance-optimization",
        "Amazon EC2 Reserved Instance Optimization",
        "Checks instance usage over time against the Reserved Instance purchases that \
         would cover it.",
        "pricing-model",
        "Perform pricing model analysis",
    ),
    check(
        "savings-plans",
        "Savings Plans",
        "Checks compute usage over time against the Savings Plans commitments that would \
         cover it.",
        "pricing-model",
        "Perform pricing model analysis",
    ),
    // Sustainability.
    check(
        "underutilized-amazon-ebs-volumes",
        "Underutilized Amazon EBS Volumes",
        "Checks volumes whose provisioned capacity and activity make them \
         over-provisioned.",
        "data-patterns",
        "Minimize over-provisioning in block storage",
    ),
];

/// One [`TrustedAdvisorCheck`], so the catalog above reads as a table.
const fn check(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    question_id: &'static str,
    best_practice: &'static str,
) -> TrustedAdvisorCheck {
    TrustedAdvisorCheck {
        id,
        name,
        description,
        question_id,
        best_practice,
    }
}

/// The checks associated to ONE best practice of ONE lens review: the catalog rows that
/// inspect the choice `choice_id` names of `question`, in the catalog's order.
///
/// Empty for a lens other than the AWS Well-Architected Framework lens: a check inspects a
/// best practice of a NAMED lens (`CheckDetail$LensArn`), and the checks this emulator
/// publishes are associated to that lens's question set, so a specialised official lens or
/// an imported custom lens has none. Empty, too, for a best practice with no row in the
/// catalog, which is most of them.
///
/// `question` is the question as the lens under review asks it, so the `ChoiceId` a check
/// is matched by is the id that question's own choice catalog names the best practice by.
pub fn checks_for_best_practice(
    lens: &str,
    question: &ReviewQuestion,
    choice_id: &str,
) -> Vec<&'static TrustedAdvisorCheck> {
    if lens_alias(lens) != FRAMEWORK_LENS_ALIAS {
        return Vec::new();
    }
    let Some(best_practice) = question
        .best_practices
        .iter()
        .find(|best_practice| best_practice.id == choice_id)
    else {
        return Vec::new();
    };
    TRUSTED_ADVISOR_CHECK_CATALOG
        .iter()
        .filter(|check| {
            check.question_id == question.question_id
                && check.best_practice == best_practice.title
        })
        .collect()
}

/// What one check can report for one account: the `CheckStatus` it carries there and the
/// `CheckFailureReason` for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckResult {
    /// `CheckDetail$Status` / `CheckSummary$Status`.
    pub status: &'static str,
    /// `CheckDetail$Reason`.
    pub reason: &'static str,
}

/// The result one check carries for one account of a workload.
///
/// A check's finding is what Trusted Advisor read from that account's resources, and this
/// emulator holds no such reading for any account — so what it reports is WHY the check has
/// no result there, which depends on whether the account is the one the workload is in:
///
/// * the account that OWNS the workload is read directly, and the Trusted Advisor checks
///   are provided by an account's support plan rather than by this emulator, so the check
///   is `NOT_AVAILABLE` for `PREMIUM_SUPPORT_REQUIRED`;
/// * any OTHER account is read by assuming a role in it, which this emulator cannot do, so
///   the fetch itself did not happen: `FETCH_FAILED` for `ASSUME_ROLE_ERROR`.
///
/// Neither is a finding about a caller's resources — the three `CheckStatus` members that
/// state one are never reported here.
pub fn check_result(account_id: &str, workload_owner: &str) -> CheckResult {
    if account_id == workload_owner {
        return CheckResult {
            status: CHECK_STATUS_NOT_AVAILABLE,
            reason: CHECK_REASON_PREMIUM_SUPPORT_REQUIRED,
        };
    }
    CheckResult {
        status: CHECK_STATUS_FETCH_FAILED,
        reason: CHECK_REASON_ASSUME_ROLE_ERROR,
    }
}

/// Every `CheckStatus` member in the model's own declared order
/// (`smithy_query(query="shape", shape="CheckStatus")`), which runs from the state that
/// reports no finding, through the findings in increasing gravity, to the two that say the
/// check produced no result for the account at all.
///
/// That order is the precedence a check's ONE status over several accounts is taken at
/// ([`check_account_summary`]): the model gives a summary a single `Status` beside the
/// per-status account counts without saying how the two relate, so the status reported is
/// the one furthest along this order that any related account carries — a summary cannot
/// claim a finding describes the whole workload while an account produced no result at all.
pub const CHECK_STATUS_BY_SEVERITY: &[&str] = &[
    "OKAY",
    "WARNING",
    "ERROR",
    CHECK_STATUS_NOT_AVAILABLE,
    CHECK_STATUS_FETCH_FAILED,
];

/// Where `status` sits in [`CHECK_STATUS_BY_SEVERITY`]; a value that order does not name
/// ranks past every one it does, so an unranked status is never hidden behind a ranked one.
fn check_status_severity(status: &str) -> usize {
    CHECK_STATUS_BY_SEVERITY
        .iter()
        .position(|ranked| *ranked == status)
        .unwrap_or(CHECK_STATUS_BY_SEVERITY.len())
}

/// One check SUMMARIZED over the accounts related to a workload: how many of them carry
/// each `CheckStatus`, and the one status the check carries for the workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckAccountSummary {
    /// `CheckSummary$Status` — the status furthest along [`CHECK_STATUS_BY_SEVERITY`] that
    /// any of the accounts carries.
    pub status: &'static str,
    /// `CheckSummary$AccountSummary` — "account summary associated to the check"
    /// (`smithy_query(query="shape", shape="CheckSummary")`): the number of accounts the
    /// check carries each status in, which is why `CheckStatusCount` is bounded at 101, one
    /// per account a workload can relate to (`WorkloadAccountIds` holds at most 100 beside
    /// the account the workload lives in). A status no account carries has no entry, so
    /// every count is at least the 1 that bound's minimum requires.
    pub accounts_by_status: std::collections::HashMap<String, i32>,
}

/// Summarize ONE check over `account_ids`, the accounts related to a workload
/// ([`super::WorkloadRecord::check_account_ids`]), from what that check reports for each of
/// them ([`check_result`]).
///
/// `None` for no account at all: a summary states a status and a per-status count, and with
/// nothing to count there is neither.
pub fn check_account_summary(
    account_ids: &[String],
    workload_owner: &str,
) -> Option<CheckAccountSummary> {
    let mut accounts_by_status: std::collections::HashMap<String, i32> =
        std::collections::HashMap::new();
    let mut status: Option<&'static str> = None;
    for account_id in account_ids {
        let result = check_result(account_id, workload_owner);
        *accounts_by_status
            .entry(result.status.to_string())
            .or_insert(0) += 1;
        if status.is_none_or(|current| {
            check_status_severity(result.status) > check_status_severity(current)
        }) {
            status = Some(result.status);
        }
    }
    status.map(|status| CheckAccountSummary {
        status,
        accounts_by_status,
    })
}
