// SPDX-License-Identifier: CC0-1.0

//! The pass-role authorization this service applies to every role ARN a request
//! asks Amazon EKS to assume, and the sentences a value that names no role of the
//! caller's own account is refused with.
//!
//! It needs nothing from the store: the question is decided from the value alone.

/// The refusal a request that asks Amazon EKS to pass a role that is not a role
/// of the caller's OWN account earns.
///
/// A fixed sentence tied to the rule — Amazon EKS authorizes the `iam:PassRole`
/// of the value a request names, and a value that does not name a role of this
/// account can never be passed, whatever its name or path — so it carries
/// nothing from the request. It is the sentence the recorded service answers,
/// including for a `roleArn` that is not an ARN at all: the pass-role is
/// evaluated against the value AS A ROLE OF SOME OTHER ACCOUNT, not parsed as a
/// parameter.
pub const CROSS_ACCOUNT_PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The refusal a `RegisterCluster` whose `connectorConfig.roleArn` is not a role
/// of this account earns.
///
/// A registration does NOT answer the pass-role refusal above, and this is the
/// one place in this service where that differs: the recorded service refuses an
/// unusable `connectorConfig.roleArn` with HTTP 400 `InvalidParameterException`
/// and this fixed sentence, while the same value in `CreateCluster$roleArn` is
/// refused 403 with [`CROSS_ACCOUNT_PASS_ROLE_REFUSAL`]. The QUESTION is the same
/// one [`pass_role_refusal`] answers — does the value name a role of this
/// account? — so one reading serves both; only how each operation reports it
/// differs, and each reports it as its own recording shows.
pub const CONNECTOR_ROLE_ARN_REFUSAL: &str = "The provided roleArn is invalid.";

/// The service principal the EKS connector role must let assume it: the cluster
/// is registered as a Systems Manager managed instance, so SSM is what assumes
/// the role `connectorConfig.roleArn` names (Amazon EKS User Guide, "Connecting
/// an external cluster"), which is also the principal the recorded refusal for
/// an unusable connector role names.
pub const CONNECTOR_SERVICE_PRINCIPAL: &str = "ssm.amazonaws.com";

/// The Amazon EKS service principal, for a role the model says Amazon EKS itself
/// assumes: "This role must have a trust policy that allows the EKS service
/// principal to assume it" (`CreateCapabilityRequest$roleArn`).
pub const EKS_SERVICE_PRINCIPAL: &str = "eks.amazonaws.com";

/// The service principal Amazon EKS assumes a managed CAPABILITY's role as.
///
/// A capability runs outside the cluster — "Amazon EKS provisions and manages the
/// infrastructure required to run the capability outside of your cluster"
/// (`CreateCapability`) — and the plane that runs it speaks for the cluster's
/// capabilities rather than for the control plane, so it is its own service
/// principal under the Amazon EKS domain.
pub const CAPABILITY_SERVICE_PRINCIPAL: &str = "capabilities.eks.amazonaws.com";

/// The service principals a role named for a managed capability may let assume
/// it: the capability plane's own principal, and the Amazon EKS control plane's.
///
/// The model states the rule as one principal ("a trust policy that allows the
/// EKS service principal to assume it", `CreateCapabilityRequest$roleArn`) and
/// Amazon EKS publishes more than one principal under that one name, so a role
/// whose trust policy admits ANY of them is a role Amazon EKS can assume for the
/// capability; a `roleArn` refusal is reserved for a role that is absent or that
/// admits none of them.
pub const CAPABILITY_ROLE_PRINCIPALS: &[&str] =
    &[CAPABILITY_SERVICE_PRINCIPAL, EKS_SERVICE_PRINCIPAL];

/// The EC2 error code for a subnet id no subnet of the account carries.
pub const EC2_INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// The partition every ARN this emulator mints carries, read off one it mints
/// rather than written out again.
pub fn emulator_partition() -> String {
    ::engine::emulator_arn("iam", "", "")
        .split(':')
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

/// Why `role_arn` is not a role this caller may have Amazon EKS assume, or
/// `None` when it is one.
///
/// A role a request hands to Amazon EKS is passed to the service with the
/// caller's own `iam:PassRole`, so the question is whether the value names a
/// role THIS caller can pass: a well-formed IAM role ARN of the caller's own
/// partition and account. The `region` field of such an ARN is empty — IAM is
/// global, so an IAM ARN carries no Region.
///
/// EVERY failure of that question is one answer, a PERMISSION failure
/// (`AccessDeniedException`, 403), never a malformed parameter: the recorded
/// service refuses a `roleArn` of `not-an-arn` with
/// [`CROSS_ACCOUNT_PASS_ROLE_REFUSAL`] and not as a bad parameter, because the
/// authorization is evaluated against the value as the role of an account that
/// is not this one. The message says which: the fixed sentence for a value that
/// names no role of this account at all, and what else was wrong for a
/// well-formed IAM ARN of this account that names something other than a role,
/// or a role of another partition.
///
/// What is NOT answered here is whether such a role EXISTS and trusts the
/// principal that would assume it: that lives in IAM, and
/// [`service_principal_trust_violation`] is what asks it, for the roles whose
/// contract names the principal.
pub fn pass_role_refusal(role_arn: &str) -> Option<String> {
    let own_account = ::engine::emulator_account_id();
    let own_partition = emulator_partition();
    // `splitn(6, ':')` leaves the whole resource — colons included — as the last field.
    let fields: Vec<&str> = role_arn.splitn(6, ':').collect();
    let [literal, partition, vendor, region, account, resource] = fields.as_slice() else {
        return Some(CROSS_ACCOUNT_PASS_ROLE_REFUSAL.to_string());
    };
    // The IAM ARN form, `arn:<partition>:iam::<12-digit account>:<resource>` — IAM is a
    // global service, so the Region segment is empty. This is the same reading
    // [`is_iam_role_arn`] applies, minus the question of WHAT the resource names, which
    // is the next step rather than part of the form.
    let is_iam_arn = *literal == "arn"
        && !partition.is_empty()
        && *vendor == "iam"
        && region.is_empty()
        && account.len() == 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && !resource.is_empty();
    // A value that is not an IAM ARN of this account names no role of it, which is the
    // same answer a role of another account gets.
    if !is_iam_arn || *account != own_account {
        return Some(CROSS_ACCOUNT_PASS_ROLE_REFUSAL.to_string());
    }
    // An IAM role's resource field is `role/<path><name>`; any other identity
    // (`user/`, `group/`, `instance-profile/`) is not something a service assumes.
    if resource.strip_prefix("role/").unwrap_or_default().is_empty() {
        return Some(format!("{role_arn} does not name an IAM role"));
    }
    if *partition != own_partition.as_str() {
        return Some(format!(
            "{role_arn} is in partition {partition}, not {own_partition}"
        ));
    }
    None
}

/// The role NAME `role_arn` names in the account this emulator serves, or `None`
/// when it is not an in-account IAM role ARN at all — the case
/// [`pass_role_refusal`] already reports, so this answers nothing about it.
///
/// IAM addresses a role by name, not by path, so the path a role sits under is
/// dropped: `role/service-role/Connector` is the role `Connector`.
pub(super) fn iam_role_name(role_arn: &str) -> Option<&str> {
    if pass_role_refusal(role_arn).is_some() {
        return None;
    }
    let resource = role_arn.splitn(6, ':').nth(5)?;
    let name = resource.strip_prefix("role/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}
