// SPDX-License-Identifier: CC0-1.0

//! The `StackStatusReason` a stack operation reports when its RESOURCES are what failed it,
//! and the verb each failure is summarised with.

use super::*;

/// The `StackStatusReason` a stack operation reports when its RESOURCES are what failed it.
///
/// The field is a free-form string, and what CloudFormation puts in it for this class of
/// failure is one SUMMARY sentence: the fixed text around the comma-separated
/// `LogicalResourceId`s of the resources the operation could not carry through, and nothing
/// else. The sentence ends with its separating space, which is where CloudFormation's own
/// follow-on clause about the stack would go — never the failing resource's diagnosis.
///
/// That diagnosis is a per-RESOURCE field: `StackEvent$ResourceStatusReason` is the
/// "success/failure message associated with the resource", so the owning service's refusal
/// (its error code, its message) is carried there, on the event for the resource that
/// failed, and is read through `DescribeStackEvents` and `DescribeStackResource`. Folding it
/// into the stack-level summary would make the one field a client parses to recover the set
/// of failed logical ids depend on provider error text instead.
///
/// `action` is what the operation was doing ([`failed_action`]).
pub fn resource_failure_summary<'a>(
    action: &str,
    logical_ids: impl IntoIterator<Item = &'a str>,
) -> String {
    let named: Vec<&str> = logical_ids.into_iter().collect();
    format!(
        "The following resource(s) failed to {action}: [{}]. ",
        named.join(", ")
    )
}

/// The verb the failure of resources a deployment was CREATING is summarised with, in the
/// deployment mode that deployment runs in.
///
/// A deployment in [`DEPLOYMENT_MODE_EXPRESS`] names its mode in the summary: that mode
/// "complete[s] the stack operation when resource configuration is applied", so a resource
/// it could not configure failed in a way the caller reads differently from a standard
/// deployment's failure — and the stack says which of the two it was. Every other mode is
/// the plain verb ([`failed_action::CREATE`]).
pub fn create_action(express: bool) -> &'static str {
    if express {
        failed_action::CREATE_VIA_EXPRESS_MODE
    } else {
        failed_action::CREATE
    }
}
