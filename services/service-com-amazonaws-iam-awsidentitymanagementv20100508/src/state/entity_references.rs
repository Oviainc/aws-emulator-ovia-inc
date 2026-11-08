// SPDX-License-Identifier: CC0-1.0

//! The sentences IAM refuses a value it cannot read as the ONE thing a member must name
//! with — an IAM identity, or the account a simulation's resources belong to.
//!
//! One concern, held in one place: the question every such member asks of its value
//! ([`names_no_single_entity`]) and the three measured sentences the operations taking one
//! state their refusal in, so no two of them word the same refusal two ways.

#[allow(unused_imports)]
use super::*;

/// The characters an IAM policy element matches a SET of values with: "the IAM policy
/// language supports the `*` and `?` wildcards" — `*` standing for any run of characters
/// and `?` for a single one. A value carrying either names a set rather than one thing.
const WILDCARDS: [char; 2] = ['*', '?'];

/// Whether `value` stands for a SET of names rather than for one thing: whether it carries
/// a wildcard anywhere ([`WILDCARDS`]).
///
/// The question every member that must name ONE thing asks of its value, whatever KIND of
/// thing that is — the account a simulation's resources belong to
/// (`SimulatePrincipalPolicy$ResourceOwner`), or a resource a supplied resource-based
/// policy is attached to (`SimulatePrincipalPolicy$ResourcePolicy`, "each resource in the
/// simulation is treated as if it had this policy attached"). Asked HERE, once, so those
/// members cannot disagree about what names one thing.
pub fn names_no_single_entity(value: &str) -> bool {
    value.contains(WILDCARDS)
}

/// How the policy SIMULATOR refuses a value supplied for a member that must name one IAM
/// user, group or role and cannot be read as one: the fixed text `Invalid ARN provided in
/// the request: ` followed by the value the request supplied.
///
/// Measured wording, and it is the simulator's sentence for every value it cannot use for
/// such a member — whether nothing of the value reads as an ARN or the ARN names another
/// resource — so the operations reading a principal or a simulated caller
/// (`SimulatePrincipalPolicy`, `SimulateCustomPolicy`,
/// `GetContextKeysForPrincipalPolicy`) cannot word that refusal two ways. The operations
/// outside that family word their own
/// ([`entity_arn_unsupported_message`]): IAM states this refusal per operation, and which
/// sentence a caller sees is part of the wire contract of the operation it called.
pub fn arn_not_provided_message(value: &str) -> String {
    format!("Invalid ARN provided in the request: {value}")
}

/// How IAM refuses a value for a member that must name ONE thing when the value stands for
/// a SET of names ([`names_no_single_entity`]): the fixed text `Invalid Entity Arn: ` and
/// ` does not clearly define entity type and name.` around the value the request supplied.
///
/// Measured wording: the refusal states that the value pins neither the type nor the name
/// of the one thing the member has to name.
pub fn entity_arn_not_one_entity_message(value: &str) -> String {
    format!("Invalid Entity Arn: {value} does not clearly define entity type and name.")
}

/// How `ListPoliciesGrantingServiceAccess` refuses an `Arn` naming no identity whose
/// policies it could list: the fixed text `Invalid Entity Arn: ` and ` is not supported.`
/// around the value the request supplied.
///
/// Measured wording, and this operation's own: see [`arn_not_provided_message`] for the
/// sentence the simulator's members carry instead.
pub fn entity_arn_unsupported_message(value: &str) -> String {
    format!("Invalid Entity Arn: {value} is not supported.")
}
