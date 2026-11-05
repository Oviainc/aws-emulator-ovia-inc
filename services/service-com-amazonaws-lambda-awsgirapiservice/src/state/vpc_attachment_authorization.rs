// SPDX-License-Identifier: CC0-1.0

//! Whether the EXECUTION ROLE of a function may do what attaching it to a VPC takes.
//!
//! "To connect a function to a VPC, Lambda creates an elastic network interface for each
//! subnet in your function's VPC configuration" ([`VpcConfig`](crate::ops::types::VpcConfig)):
//! those interfaces are not created by whoever sent the request but by Lambda ACTING AS the
//! function's own execution role — "the Amazon Resource Name (ARN) of the function's
//! execution role" (`FunctionConfiguration$Role`). A role that may not make those EC2 calls
//! therefore describes a VPC attachment that can never be stood up, so the question is
//! settled by the call that WRITES the configuration, before anything is persisted: a
//! customer learns the role is short of a grant from the refusal instead of from an
//! invocation that fails with no network.
//!
//! The pieces are the ones [`super::event_source_authorization`] already holds for the other
//! half of this question — what a role may do to the source a mapping reads — and this
//! module is the same reading applied to the calls a VPC attachment takes:
//!
//! 1. **who** — the execution role the function would run as: the one the request names when
//!    it replaces the role, and otherwise the one the stored configuration carries;
//! 2. **what the attachment takes** — [`NETWORK_INTERFACE_ACTIONS`], the EC2 network
//!    interface calls Lambda makes to attach a function to a VPC and to clean up after it,
//!    on the network interfaces of this account's own Region;
//! 3. **what the role may do** — the identity-based documents the IAM store holds for it,
//!    read through IAM's own accessor so this service sees exactly what `CreateRole` /
//!    `PutRolePolicy` wrote, plus the permissions boundary that caps them;
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of policy documents against a request.
//!
//! Nothing is refused on evidence this emulator does not have, exactly as the sibling module
//! states: an engine carrying no IAM store, a role ARN the store holds no role for, and a
//! role carrying an attachment whose document this account does not hold all show nothing
//! about the role and let the configuration through.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::event_source_authorization::{iam_state, IamState};
use crate::ops::types::VpcConfig;

/// The EC2 calls attaching a function to a VPC takes, in the order a refusal reports them.
///
/// These are the network interface actions AWS's own managed execution-role policy for VPC
/// access (`AWSLambdaVPCAccessExecutionRole`) grants: Lambda creates an interface per subnet
/// when the configuration is applied, reads the interfaces back, and deletes them when the
/// function no longer needs them. The decision is made on each of them, because a role
/// granted only some of them cannot carry the attachment through its whole life.
const NETWORK_INTERFACE_ACTIONS: [&str; 3] = [
    "CreateNetworkInterface",
    "DescribeNetworkInterfaces",
    "DeleteNetworkInterface",
];

/// The IAM action namespace those calls are named in, which is EC2's own.
const EC2_NAMESPACE: &str = "ec2";

/// The resource those calls are made on: the network interfaces of this account in the
/// Region the function is configured in, which is what Lambda creates an endpoint interface
/// as. Named as a resource rather than as `*` so a role scoped to this account's own
/// interfaces is accepted, the way IAM evaluates the call itself.
fn network_interface_resource(region: &str) -> String {
    ::engine::emulator_arn(EC2_NAMESPACE, region, "network-interface/*")
}

/// How Lambda words the refusal of a role that may not make one of those calls.
///
/// "The provided execution role does not have permissions to call CreateNetworkInterface on
/// EC2" — the sentence is fixed text apart from the CALL the role may not make, which is the
/// first of [`NETWORK_INTERFACE_ACTIONS`] the role is short of.
fn message(withheld: &str) -> String {
    format!("The provided execution role does not have permissions to call {withheld} on EC2")
}

/// The sentence Lambda refuses the VPC configuration `vpc_config` on a function running as
/// `role_arn` with, because the policies in force on that role withhold one of the calls the
/// attachment takes — or `None` when the role may make all of them, when the configuration
/// attaches the function to nothing at all, or when nothing of the kind can be shown (see the
/// module docs).
///
/// "To remove the function's VPC configuration, pass an empty list" together with an empty
/// list of security groups (`UpdateFunctionConfiguration`): such a configuration stands up no
/// network interface, so it takes no grant and is never refused for one.
///
/// A READ of both stores: it decides, and changes nothing. The caller refuses with the
/// modelled `InvalidParameterValueException` and persists nothing, which is what makes the
/// refusal and the attachment the only two outcomes of such a request.
#[must_use]
pub fn unattachable_vpc_config<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    role_arn: &str,
    vpc_config: &VpcConfig,
) -> Option<String> {
    if !attaches(vpc_config) {
        return None;
    }
    let iam = iam_state(engine)?;
    let withheld = withheld_action(&iam.lock(), region, role_arn)?;
    Some(message(withheld))
}

/// Whether `vpc_config` asks for the function to be attached to a VPC at all — whether it
/// names a subnet to create an interface in, or a security group to apply to one.
fn attaches(vpc_config: &VpcConfig) -> bool {
    let named = |ids: Option<&Vec<String>>| ids.is_some_and(|ids| !ids.is_empty());
    named(vpc_config.subnet_ids.as_ref()) || named(vpc_config.security_group_ids.as_ref())
}

/// The FIRST of [`NETWORK_INTERFACE_ACTIONS`] the role `role_arn` may not perform on this
/// account's network interfaces, or `None` when it may perform all of them — or when nothing
/// of the kind can be shown.
fn withheld_action(iam: &IamState, region: &str, role_arn: &str) -> Option<&'static str> {
    // The role itself first: a value this account's IAM holds no role under says nothing
    // about what that role may do.
    iam.roles.values().find(|role| role.arn == role_arn)?;
    // The documents in force on the role, read ONCE: the calls below are asked over the same
    // documents, and the collection is bounded by IAM's own quotas on how many policies one
    // role carries.
    let policies = iam.principal_policies(role_arn)?;
    let documents = iam::prepare_documents(&policies.documents);
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant to an entity": the boundary CAPS what those policies grant and grants nothing
    // itself. A role carrying none — or one whose document is not held, a ceiling this
    // emulator cannot read — is capped by nothing.
    let boundary = iam
        .permissions_boundary_document(role_arn)
        .map(|document| iam::prepare_documents(::std::slice::from_ref(&document)));
    let resource = network_interface_resource(region);
    NETWORK_INTERFACE_ACTIONS.into_iter().find(|name| {
        let action = format!("{EC2_NAMESPACE}:{name}");
        let request = iam::Request::new(&action, &resource, &[]);
        let stated = iam::identity_decision_prepared(&documents, &request);
        // An attachment whose document this account does not hold is a grant this emulator
        // cannot read, so it cannot be the basis of a refusal — but it cannot overturn one
        // either: an explicit `Deny` in a document that IS held wins over every grant.
        let granted = stated == Decision::Allowed
            || (stated != Decision::Denied && !policies.uncarried.is_empty());
        // A call an identity-based policy explicitly DENIES is already withheld on that deny,
        // so no ceiling is read for it.
        let capped = if stated == Decision::Denied {
            Decision::Allowed
        } else {
            boundary.as_ref().map_or(Decision::Allowed, |boundary| {
                iam::identity_decision_prepared(boundary, &request)
            })
        };
        !granted || capped != Decision::Allowed
    })
}

#[cfg(test)]
mod tests {
    use super::{attaches, message, network_interface_resource, NETWORK_INTERFACE_ACTIONS};
    use crate::ops::types::VpcConfig;

    /// A configuration asks for an attachment when it names a subnet or a security group, and
    /// one that names neither — however it spells "none" — takes no network interface and so
    /// no grant.
    #[test]
    fn only_a_configuration_naming_a_subnet_or_a_group_asks_for_an_attachment() {
        // (case, the configuration, whether it asks for an attachment)
        let rows: [(&str, VpcConfig, bool); 5] = [
            ("no member at all", VpcConfig::default(), false),
            (
                "both lists empty, which removes the configuration",
                VpcConfig {
                    subnet_ids: Some(Vec::new()),
                    security_group_ids: Some(Vec::new()),
                    ipv6_allowed_for_dual_stack: None,
                },
                false,
            ),
            (
                "one subnet",
                VpcConfig {
                    subnet_ids: Some(vec!["subnet-0a1b2c3d4e5f60718".to_string()]),
                    security_group_ids: Some(Vec::new()),
                    ipv6_allowed_for_dual_stack: None,
                },
                true,
            ),
            (
                "one security group and no subnet",
                VpcConfig {
                    subnet_ids: None,
                    security_group_ids: Some(vec!["sg-0a1b2c3d4e5f60718".to_string()]),
                    ipv6_allowed_for_dual_stack: None,
                },
                true,
            ),
            (
                "two subnets and a group",
                VpcConfig {
                    subnet_ids: Some(vec![
                        "subnet-0a1b2c3d4e5f60718".to_string(),
                        "subnet-0a1b2c3d4e5f60719".to_string(),
                    ]),
                    security_group_ids: Some(vec!["sg-0a1b2c3d4e5f60718".to_string()]),
                    ipv6_allowed_for_dual_stack: Some(true),
                },
                true,
            ),
        ];
        for (case, vpc_config, expected) in rows {
            let asked = attaches(&vpc_config);
            assert_eq!(
                asked, expected,
                "case {case:?}: expected it to ask for an attachment: {expected}, got {asked}"
            );
        }
    }

    /// The sentence is fixed text apart from the call the role may not make, for every call
    /// the attachment takes.
    #[test]
    fn the_refusal_names_the_call_the_role_may_not_make() {
        for action in NETWORK_INTERFACE_ACTIONS {
            let expected = format!(
                "The provided execution role does not have permissions to call {action} on EC2"
            );
            let worded = message(action);
            assert_eq!(
                worded, expected,
                "case {action:?}: expected {expected:?}, got {worded:?}"
            );
        }
    }

    /// The decision is made on the network interfaces of this account in the Region the
    /// function is configured in, so a Region is part of the resource and the account is this
    /// emulator's own.
    #[test]
    fn the_resource_is_this_accounts_network_interfaces_in_the_region() {
        for region in ["us-east-1", "eu-west-1"] {
            let resource = network_interface_resource(region);
            let expected = format!(
                "arn:aws:ec2:{region}:{}:network-interface/*",
                ::engine::emulator_account_id()
            );
            assert_eq!(
                resource, expected,
                "case {region:?}: expected {expected:?}, got {resource:?}"
            );
        }
    }
}
