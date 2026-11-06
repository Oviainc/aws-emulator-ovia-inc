// SPDX-License-Identifier: CC0-1.0

//! What the resource-policy ARN resolution SAYS: the sentence a request naming
//! no policy holder is refused with, and the line an undecided resolution
//! leaves for an operator.
//!
//! The resolution itself lives beside the ARN classification it extends, in
//! [`super::resolve_policy_resource`], so one file holds the whole decision
//! about which ARN names a policy holder — and the only thing split out here is
//! the wording, which the write, the read and the delete all answer with.

/// Why a resource-policy request's `ResourceArn` names no resource a policy can
/// be attached to, naming the ARN the request carried.
///
/// The ARN is well formed and names a service whose resources carry an Amazon
/// Redshift resource policy — the value is refused because the resource it
/// identifies is not there, which is a fact about the parameter and not about
/// the policy.
pub fn no_policy_resource_message(resource_arn: &str) -> String {
    format!("Invalid value for ResourceArn: '{resource_arn}'. The resource it names does not exist.")
}

/// Record that the resource a resource-policy request names could not be
/// resolved, so a request admitted because nothing could be asked does not read
/// like one admitted because the resource is really there.
///
/// Visible at default verbosity, for the reason the serverless registration
/// lookup logs the same condition: the degraded path says so rather than passing
/// quietly.
pub(super) fn policy_resource_undecided(resource_arn: &str, reason: &str) {
    ::tracing::warn!(
        resource_arn = %resource_arn,
        reason = %reason,
        "the resource a resource-policy request names could not be resolved"
    );
}
