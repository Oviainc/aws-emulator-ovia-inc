// SPDX-License-Identifier: CC0-1.0

//! WHAT the account-level Incident Manager precondition says when it refuses.
//!
//! The gate itself — whether the account holds an ACTIVE replication set — is
//! [`super::ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set`] and
//! [`super::ComAmazonawsSsmcontactsSsmcontactsState::require_active_replication_set_for`].
//! This module is the one place its WORDING is decided, so no two gated operations can word
//! the same failure differently:
//!
//! * a rotation and an engagement are addressed THROUGH the account's replication set, so
//!   while the account holds no set an ARN naming one resolves to nothing and the refusal
//!   reports that ARN ([`invalid_resource_arn_message`]);
//! * a call addressing anything else of this service — a contact, a contact channel, a page —
//!   or naming no resource at all is refused about the account
//!   ([`ACCOUNT_NOT_FOUND_MESSAGE`]).
//!
//! Both are rendered by the calling operation as its own modelled `ValidationException`
//! (HTTP 400). A handler passes its subject ARN only when the member it took it from may
//! name one of the first group — `RotationId`, `EngagementId`, and the tagging trio's
//! `ResourceARN`, which the model documents as the ARN of "a resource of this service", so
//! there the value itself decides; a handler whose member names a contact, a channel or a
//! page asks for the account wording directly and has nothing to decide.
//!
//! The ARN in the message is the one the request carried, read but never parsed by hand:
//! `service_common::arn` is the workspace's single ARN parse, and the only thing asked of it
//! here is the vendor and the resource TYPE.

/// The message the contact data plane refuses a request from an account with no active
/// Incident Manager replication set with, when the refusal reports the ACCOUNT.
///
/// HTTP 400 with the error code `ValidationException` (never a 404) and this wording are all
/// part of the refusal. It names the account rather than any member of the request, and it
/// carries no value taken from the request or from one run.
pub const ACCOUNT_NOT_FOUND_MESSAGE: &str =
    "Invalid value provided - Account not found for the request";

/// The resource families Incident Manager addresses THROUGH the account's replication set:
/// it has to resolve such an ARN against the set before it can say anything about the
/// resource, so on an account that holds no set the ARN resolves to nothing and the refusal
/// is worded about the ARN rather than about the account.
///
/// Spelled as the `resource` field of the ARNs this service mints for those families
/// ([`super::rotation_arn`], [`super::engagement_arn`]).
const REPLICATION_SET_RESOLVED_RESOURCE_TYPES: [&str; 2] = ["rotation", "engagement"];

/// The message the CLOSED gate refuses a request with: the ARN-resolution wording when the
/// request addressed a resource whose family is resolved through the replication set
/// ([`REPLICATION_SET_RESOLVED_RESOURCE_TYPES`]), else the account wording.
///
/// One function for the whole gated surface, so two operations that name the same kind of
/// resource cannot word the same failure differently, and a call that names no resource at
/// all (`None`) is refused about the account.
#[must_use]
pub fn closed_gate_message(subject_arn: Option<&str>) -> String {
    match subject_arn.filter(|arn| is_replication_set_resolved_arn(arn)) {
        Some(arn) => invalid_resource_arn_message(arn),
        None => ACCOUNT_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// Whether `arn` is an ARN of THIS service naming a resource of a family the gate words its
/// refusal from.
///
/// Read through `service_common::arn`, the workspace's one ARN parse, so the vendor and the
/// resource type are read the way every other service reads them — no splitting or indexing
/// of caller-supplied text here. A value that is not an ARN, or an ARN of another vendor,
/// names no such resource and leaves the account wording.
fn is_replication_set_resolved_arn(arn: &str) -> bool {
    ::service_common::arn::parse_of(arn, super::ARN_NAMESPACE).is_some_and(|parsed| {
        REPLICATION_SET_RESOLVED_RESOURCE_TYPES.contains(&parsed.resource_type())
    })
}

/// The message an operation refuses an ARN with when that ARN names no resource of this
/// service: it cannot be resolved into a resource identity at all.
///
/// Built from the request, so the ARN it names is the one the caller supplied — appended
/// whole, never reformatted. It is what the closed account gate answers for the families
/// listed in [`REPLICATION_SET_RESOLVED_RESOURCE_TYPES`] as well, since on an account with
/// no replication set no such ARN resolves to anything.
#[must_use]
pub fn invalid_resource_arn_message(arn: &str) -> String {
    format!("Invalid value provided - Invalid resource Arn {arn}")
}
