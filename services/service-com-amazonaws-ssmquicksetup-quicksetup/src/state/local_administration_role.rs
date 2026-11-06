// SPDX-License-Identifier: CC0-1.0

//! Whether the role a LOCAL configuration deployment is administered with is one that
//! deployment can be run as.
//!
//! A configuration definition that deploys locally ([`super::deploys_locally`]) is
//! administered with the role `LocalDeploymentAdministrationRoleArn` names: "The ARN of
//! the IAM role used to administrate local configuration deployments"
//! (`smithy_query(query="shape", shape="ConfigurationDefinitionInput")`). Quick Setup
//! deploys a configuration definition as a CloudFormation stack set, so the service that
//! has to act as that role is [`DEPLOYMENT_SERVICE_PRINCIPAL`] — and a request is served
//! only when the account's own IAM records SHOW that role trusting it for
//! `sts:AssumeRole`. Anything else is a local deployment that could never be
//! administered, so the request is refused before anything is stored, with
//! [`trust_policy_message`]:
//!
//! * the account holds no role under that ARN — nothing names the role the definition
//!   would be deployed as, and a local deployment's administration role belongs to the
//!   account the deployment runs in;
//! * the role carries no trust document, or one that cannot be read as a policy at all —
//!   neither shows the assume being granted;
//! * the document is a policy that withholds the assume from the principal, or denies it
//!   explicitly.
//!
//! That is the same standard every other outcome of this service is held to: a manager is
//! stored only when the state it is backed by really exists in the component that owns
//! it, which for an IAM role is IAM.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **the role** — resolved by ARN against the account's IAM records, the very records
//!    `CreateRole` wrote, read through the IAM service's own state struct in the SAME
//!    shared engine this service's store lives in;
//! 2. **the decision** — [`::service_common::iam_trust_policy`], this tree's ONE reading
//!    of a trust document, so this service and every other one handed the same document
//!    answer alike: a statement must name the principal and `sts:AssumeRole`, an explicit
//!    `Deny` wins over any `Allow`, and the document is percent-decoded before it is
//!    parsed.
//!
//! One case is left unjudged, because it is about this emulator rather than about the
//! role: an engine carrying no IAM store at all cannot be asked, and a deployment with
//! no IAM to ask has no record of any role to resolve, so no request naming one could
//! ever be served. That is the absence of the asking capability, not evidence about the
//! role.

/// The IAM store a `LocalDeploymentAdministrationRoleArn` is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so the role read here is the one IAM's handlers wrote rather
/// than a second copy of it.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The service principal that has to be able to assume a local deployment's
/// administration role: Quick Setup deploys a configuration definition as a
/// CloudFormation stack set, so CloudFormation is what acts as that role.
pub const DEPLOYMENT_SERVICE_PRINCIPAL: &str = "cloudformation.amazonaws.com";

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied. `None` means the
/// engine carries no such store — the one case this module leaves unjudged (see the
/// module docs).
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The sentence this service refuses a local deployment whose administration role cannot
/// be assumed with.
///
/// Fixed text around [`DEPLOYMENT_SERVICE_PRINCIPAL`] and the action asked of the trust
/// document: the refusal names the member's own role and what could not be done with it,
/// and renders nothing the request carried — not the ARN, not the account, not the
/// definition.
pub fn trust_policy_message() -> String {
    format!(
        "LocalDeploymentAdministrationRole trust policy does not allow AssumeRole for \
         {DEPLOYMENT_SERVICE_PRINCIPAL}."
    )
}

/// Whether `iam` shows the role `role_arn` trusting [`DEPLOYMENT_SERVICE_PRINCIPAL`] for
/// `sts:AssumeRole`.
///
/// `false` for every case the module docs list, which are all the cases in which the
/// account's records do not show the grant.
pub fn admits_deployer(role_arn: &str, iam: &IamState) -> bool {
    let Some(role) = iam.roles.values().find(|role| role.arn == role_arn) else {
        return false;
    };
    let Some(document) = role.assume_role_policy_document.as_deref() else {
        return false;
    };
    // The rule itself is IAM's, not this service's, so it is read through the ONE
    // evaluator every service that resolves a caller-supplied service role shares. A
    // document this emulator cannot read as a policy at all shows no grant either.
    ::service_common::iam_trust_policy::admits_service_principal(
        document,
        DEPLOYMENT_SERVICE_PRINCIPAL,
    ) == Some(true)
}

/// Why the local deployment of a definition holding `parameters` and naming
/// `administration_role_arn` cannot be administered, or `None` when nothing of the kind
/// is shown.
///
/// `None` for a definition that does not deploy locally ([`super::deploys_locally`]): the
/// role is what a LOCAL deployment is administered with, so a definition deployed through
/// an organization is not held to it. `None` too for one naming no role at all, which
/// [`super::local_deployment_rejection`] is what refuses, and for an engine carrying no
/// IAM store (see the module docs).
pub fn deployment_role_rejection<P, Eng>(
    engine: &Eng,
    parameters: &P,
    administration_role_arn: Option<&str>,
) -> Option<String>
where
    P: super::ConfigurationParameters + ?Sized,
    Eng: Send + Sync + 'static,
{
    if !super::deploys_locally(parameters) {
        return None;
    }
    let role_arn = administration_role_arn?;
    let iam = iam_state(engine)?;
    let held = iam.lock();
    (!admits_deployer(role_arn, &held)).then(trust_policy_message)
}
