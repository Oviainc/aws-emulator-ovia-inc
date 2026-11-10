// SPDX-License-Identifier: CC0-1.0

//! Whether this service can still assume a scheduled report's execution role.
//!
//! A scheduled report does not run as whoever created it: the model documents
//! `ScheduledReportInput$scheduledReportExecutionRoleArn` as "the ARN of the IAM role that
//! the scheduled report uses to execute. Amazon Web Services Billing and Cost Management
//! Dashboards will assume this IAM role while executing the scheduled report". So a role
//! whose trust policy stops admitting this service is a report that cannot execute at all,
//! which is what `HealthStatus$statusCode` calls the unhealthy state — `HEALTHY` is
//! documented as the report being "configured properly and has all required permissions to
//! execute" — with `EXECUTION_ROLE_ASSUME_FAILED` among its `statusReasons`.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the stored record's own `execution_role_arn`, read off the record at the
//!    moment of each health resolution, so a report whose role `UpdateScheduledReport` has
//!    switched is judged by the role it carries then;
//! 2. **what the account's IAM says about it** — the trust document the IAM store holds for
//!    that role, which is the very document `CreateRole` and `UpdateAssumeRolePolicy` wrote,
//!    reached through the SAME shared engine this service's store lives in;
//! 3. **the decision** — [`::service_common::iam_trust_policy::admits_service_principal`],
//!    the tree's ONE reading of a trust document, so this service and every other one handed
//!    the same document answer alike.
//!
//! Nothing is reported on evidence this emulator does not have. Each of these shows nothing
//! about the role and leaves the report's health to the rest of the derivation: an engine
//! carrying no IAM store, a `scheduledReportExecutionRoleArn` the store holds no role for
//! (an execution role is routinely provisioned out of band, so "this emulator's IAM holds no
//! such role" is evidence about the store and not about the role — the same reading
//! `service_common::iam_role::held_role_is_unassumable` takes for an execution role), a role
//! carrying no trust document, and a document that cannot be read as a policy.
//!
//! The judgement is taken as a SNAPSHOT of the IAM store ([`UnassumableExecutionRoles`])
//! rather than per report: a listing resolves the health of a whole page of reports, and one
//! pass over the account's roles answers for every one of them, with the IAM guard released
//! before this service's own store is read.

use std::collections::BTreeSet;

/// The IAM store a scheduled report's execution role ARN is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so a scheduled report is judged against the very records IAM's
/// handlers wrote rather than against a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live. `None`
/// means the engine carries no such store — a deployment with no IAM to ask, which is
/// evidence of nothing.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The service principal a scheduled report's execution role has to trust for this service
/// to assume it, as a trust document names one: this service's own endpoint prefix under
/// `amazonaws.com`, which is how AWS names a service principal.
pub const SERVICE_PRINCIPAL: &str = "bcm-dashboards.amazonaws.com";

/// The execution roles the account's IAM shows this service CANNOT assume, as of the call
/// that resolved them.
///
/// Re-resolved on every health resolution and never cached on a record: a trust policy is
/// edited outside this service, so the verdict is a property of the IAM store at the instant
/// of the call — which is also what makes a repaired trust policy restore a report's health
/// with no further call to this service.
#[derive(Debug, Default, Clone)]
pub struct UnassumableExecutionRoles {
    /// The ARN of every role the store HOLDS whose trust document withholds
    /// [`::service_common::iam_trust_policy::ASSUME_ROLE_ACTION`] from
    /// [`SERVICE_PRINCIPAL`]. A role the store does not hold, or whose document shows
    /// nothing, is absent — see the module docs.
    unassumable: BTreeSet<String>,
}

impl UnassumableExecutionRoles {
    /// The roles the IAM store of `engine` shows this service cannot assume.
    ///
    /// The IAM guard is taken and released here, BEFORE this service's own store is read, so
    /// no caller holds two stores at once.
    pub fn resolve<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        let Some(iam) = iam_state(engine) else {
            // No IAM in this emulator to ask: nothing is shown about any role.
            return Self::default();
        };
        let held = iam.lock();
        Self::of(&held)
    }

    /// The same reading against an IAM store the caller already holds.
    ///
    /// One pass over the account's roles — bounded by IAM's own store rather than by a
    /// number chosen here — so resolving the health of a page of reports costs one walk and
    /// then a lookup per report.
    pub fn of(iam: &IamState) -> Self {
        let unassumable = iam
            .roles
            .values()
            .filter(|role| {
                role.assume_role_policy_document
                    .as_deref()
                    .and_then(|document| {
                        ::service_common::iam_trust_policy::admits_service_principal(
                            document,
                            SERVICE_PRINCIPAL,
                        )
                    })
                    == Some(false)
            })
            .map(|role| role.arn.clone())
            .collect();
        Self { unassumable }
    }

    /// Whether the account's IAM shows that this service cannot assume `role_arn`, so a
    /// report carrying it has nothing it could execute as.
    pub fn assume_failed(&self, role_arn: &str) -> bool {
        self.unassumable.contains(role_arn)
    }
}
