// SPDX-License-Identifier: CC0-1.0

//! The ARN SYNTAX the tagging surface reads a caller-supplied `resourceArn` against.
//!
//! The three tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`)
//! address their resource by an ARN the caller supplies, and the model gives that member
//! no identifier syntax of its own (`LongStringAttribute`, `@length(1..2048)`,
//! `^[^<>]*$`), so the boundary validator admits any text without angle brackets. A value
//! that is not a well-formed ARN therefore reaches the service, where it is a REQUEST-SHAPE
//! failure and not an authorization one: there is no resource ARN to authorize, so it is
//! refused with the front door's own 400 request shape
//! ([`crate::scaffolded::FrontDoorRefusal::invalid_resource_arn`]) before any lookup
//! resolves a target — ahead of that door's 401, which is for an ARN this surface CAN read
//! and that names nothing this account holds.
//!
//! The ARN form itself is not spelled out here: it is the one
//! [`service_common::arn::parse`] reads for every service that binds a caller-supplied ARN
//! (`arn:<partition>:<vendor>:<region>:<account>:<resource>`, with the partition, the
//! vendor and the resource non-empty and the Region and account allowed to be empty), so
//! this service cannot disagree with its neighbours about what an ARN is.

/// Whether `resource_arn` is a value the tagging surface can read an ARN out of at all.
///
/// The question a tag call is judged by BEFORE its target is resolved. Only the syntax is
/// asked about: which resource the ARN names — this service's or another's, this account's
/// or another's, one this account holds or none — is a question about state, and it is
/// answered by the lookup each handler then performs
/// ([`super::ComAmazonawsArtifactArtifactState::tagged_resource`]) with the one 401 that
/// discloses nothing.
pub fn is_resource_arn(resource_arn: &str) -> bool {
    ::service_common::arn::parse(resource_arn).is_some()
}
