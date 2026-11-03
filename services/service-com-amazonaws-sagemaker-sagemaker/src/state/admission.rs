// SPDX-License-Identifier: CC0-1.0

//! The ADMISSION slice of this service's state: the questions a SageMaker create answers
//! about the identity, the references and the network a request names BEFORE it stores
//! anything, and the sentences it refuses each of them with.
//!
//! The three facts a create settles are stated once in this module so no two of them word
//! the same refusal differently:
//!
//! * the execution role the resource would RUN AS ([`execution_role_is_unassumable`]);
//! * the network placement it asks for ([`SECURITY_GROUPS_NOT_SPECIFIED`]).
//!
//! Each refusal is 400 `ValidationException` — the class recorded SageMaker answers a
//! request it understood and will not accept — carried through
//! [`crate::scaffolded::Scaffolded::validation`], because none of these operations models
//! an error for a bad request.
//!
//! # Only what IAM ANSWERED is refused
//!
//! The role is a resource of ANOTHER service, reached the way any emulated service reaches
//! another: the AWS SDK over the emulator's OWN in-process callback, through the shared
//! reading in `service_common`. The question is not answered from a guess: an emulator with
//! no IAM mounted to ask, and any answer other than "this account holds no such role",
//! refuses NOTHING. It is awaited with NO store guard held.

/// The service principal SageMaker assumes a caller-supplied execution role as, from
/// `CreateNotebookInstanceInput$RoleArn`'s doc: "you must grant the SageMaker AI service
/// principal (sagemaker.amazonaws.com) permissions to assume this role".
pub const SERVICE_PRINCIPAL: &str = "sagemaker.amazonaws.com";

/// How SageMaker words a `RoleArn` it cannot assume: the role ARN AS THE CALLER SENT IT,
/// then the fixed sentence naming what the role has to allow.
///
/// Stated once here so every create that runs a resource as a caller-supplied role words
/// the same fact the same way.
#[must_use]
pub fn could_not_assume_role(role_arn: &str) -> String {
    format!(
        "Could not assume role {role_arn}. Please ensure that the role exists and allows \
         principal '{SERVICE_PRINCIPAL}' to assume the role."
    )
}

/// Whether this emulator's own IAM ANSWERED that `role_arn` is a role SageMaker cannot
/// assume: this account holds no such role, or the trust policy it carries does not admit
/// [`SERVICE_PRINCIPAL`].
///
/// A job and a notebook instance each RUN AS the role the request names, so a role
/// SageMaker cannot assume backs no work at all and the create is refused with
/// [`could_not_assume_role`] before anything is stored. The whole question — which role of
/// this account the value names, what IAM answers about it, and whether the trust document
/// admits a service principal — is the ONE reading every emulated service that resolves a
/// caller-supplied execution role shares
/// (`service_common::iam_role::role_is_unassumable`), so this module states only WHICH
/// principal has to be admitted and WHICH operations ask.
pub async fn execution_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL).await
}

/// How SageMaker words a request that places a resource in a VPC subnet and names no
/// security group for it: a FIXED sentence carrying nothing from the request.
///
/// `SecurityGroupIds`' doc: "The VPC security group IDs, in the form sg-xxxxxxxx. The
/// security groups must be for the same VPC as specified in the subnet" — a placement in a
/// subnet is incomplete without one, and the member is modelled optional because the
/// placement itself is, so the pair is this service's own rule.
pub const SECURITY_GROUPS_NOT_SPECIFIED: &str = "Check if the security groups are specified";

/// Why the network placement `subnet_id` / `security_group_ids` asks for is one SageMaker
/// will not accept, or `None` when it is complete.
///
/// A request that names NO subnet asks for no VPC placement at all, so it needs no
/// security group; one that names a subnet needs at least one.
#[must_use]
pub fn vpc_placement_refusal(
    subnet_id: Option<&str>,
    security_group_ids: Option<&[String]>,
) -> Option<&'static str> {
    let placed_in_a_subnet = subnet_id.is_some_and(|subnet| !subnet.is_empty());
    let named_a_group = security_group_ids.is_some_and(|groups| !groups.is_empty());
    (placed_in_a_subnet && !named_a_group).then_some(SECURITY_GROUPS_NOT_SPECIFIED)
}

#[cfg(test)]
mod tests {
    use super::vpc_placement_refusal;

    /// One placement a request can ask for, and whether this service refuses it.
    struct Row {
        case: &'static str,
        subnet: Option<&'static str>,
        groups: Option<Vec<String>>,
        refused: bool,
    }

    /// RULE: a VPC placement is refused exactly when a subnet is named and no security
    /// group is.
    #[test]
    fn a_subnet_placement_needs_a_security_group() {
        let group = || Some(vec!["sg-0123456789abcdef0".to_string()]);
        let rows = vec![
            Row {
                case: "no VPC placement at all",
                subnet: None,
                groups: None,
                refused: false,
            },
            Row {
                case: "a subnet and a group",
                subnet: Some("subnet-1"),
                groups: group(),
                refused: false,
            },
            Row {
                case: "a subnet and no group member",
                subnet: Some("subnet-1"),
                groups: None,
                refused: true,
            },
            Row {
                case: "a subnet and an empty group list",
                subnet: Some("subnet-1"),
                groups: Some(Vec::new()),
                refused: true,
            },
            Row {
                case: "groups but no subnet",
                subnet: None,
                groups: group(),
                refused: false,
            },
            Row {
                case: "an empty subnet id",
                subnet: Some(""),
                groups: None,
                refused: false,
            },
        ];

        for row in rows {
            let refused = vpc_placement_refusal(row.subnet, row.groups.as_deref()).is_some();
            assert!(
                refused == row.refused,
                "case {:?}: expected refused={}, got {refused}",
                row.case,
                row.refused
            );
        }
    }
}
