// SPDX-License-Identifier: CC0-1.0

//! The sentences Amazon EMR refuses a MODIFICATION with: a change to a job flow that is
//! on its way out, a scaling policy on a group or with limits that cannot carry one, and
//! a tag call that names no tag.
//!
//! Each of these refusal classes is answered with ONE sentence of entirely fixed text:
//! Amazon EMR interpolates nothing of the request or of the store into any of them, so a
//! caller matching the sentence recognises every request of the class the same way. They
//! live here, in one place per class, so the operations that share a class cannot come to
//! word it differently — [`job_flow_not_modifiable_message`] is answered by every
//! operation of the legacy job-flow surface that changes a cluster, and
//! [`invalid_managed_scaling_compute_limits_message`] by every path that attaches a
//! managed scaling policy.

/// How the legacy job-flow surface refuses a change to a cluster that is shutting down or
/// gone: one fixed sentence.
///
/// Recorded behavior for the text, and it is CONSTANT by design — neither the cluster id
/// the request named nor the state the store holds it in is part of it, so every operation
/// of the surface refuses every such cluster with the same sentence.
pub fn job_flow_not_modifiable_message() -> String {
    "A job flow that is shutting down, terminated, or finished may not be modified.".to_string()
}

/// How `PutAutoScalingPolicy` refuses the instance group that runs the primary node:
/// automatic scaling is "for a core instance group or task instance group" (that
/// operation's doc, Smithy model), so the primary group carries no policy at all.
///
/// Recorded behavior for the text: the sentence is FIXED — the group the request named is
/// no part of it — and it is answered whatever else the submitted policy carries, because
/// the group is judged before the policy is read.
pub fn master_instance_group_auto_scaling_message() -> String {
    "Auto Scaling policy should not be specified for Master Instance Groups.".to_string()
}

/// How every path that attaches a managed scaling policy refuses compute limits whose
/// lower boundary is above the upper one.
///
/// `MinimumCapacityUnits` is "the lower boundary of Amazon EC2 units" and
/// `MaximumCapacityUnits` "the upper boundary of Amazon EC2 units" (`ComputeLimits` member
/// docs, Smithy model), so such limits name no range a scaling activity could stay inside.
/// Recorded behavior for the text, which is fixed: neither capacity is interpolated.
pub fn invalid_managed_scaling_compute_limits_message() -> String {
    "Invalid configuration for managed scaling. The specified max capacity must be greater \
     than min capacity."
        .to_string()
}

/// How `AddTags` refuses a request that carries no tag to add.
///
/// `Tags` is "a list of tags to associate with a resource" and is `@required`
/// (`AddTags$Tags`, Smithy model); the model bounds its length nowhere, so a list that is
/// present and EMPTY reaches the service, names no tag to associate, and is refused by
/// this fixed sentence rather than answered as a change that happened.
pub fn empty_tag_list_message() -> String {
    "Tags cannot be null or empty.".to_string()
}
