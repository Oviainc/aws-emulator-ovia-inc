// SPDX-License-Identifier: CC0-1.0

//! Whether the account this emulator serves holds an Amazon Web Services Premium Support
//! subscription, which is what `CreateEnvironment` requires of it.
//!
//! An Amazon EVS environment runs licensed VCF software under a Premium Support
//! subscription, so an account that holds none cannot create one: Amazon EVS refuses the
//! request with the entitlement clause [`ENTITLEMENT_CLAUSE`], collected ahead of
//! everything else the request is judged on, however well formed the rest of it is.
//!
//! The subscription is NOT this service's fact: it is the account's Support plan, which
//! the Amazon Web Services Support emulator owns and an operator grants out of band
//! (`SUPPORT_PLAN=enterprise`, that crate's
//! [`state::SUPPORT_PLAN_ENV`](::service_com_amazonaws_support_awssupport_20130415::state::SUPPORT_PLAN_ENV)).
//! So the question is put to the Support store's OWN accessor in the shared engine —
//! exactly as the network a request names is put to Amazon EC2's
//! ([`super::ec2_resources`]) — rather than kept as a second copy here, and one plan
//! serves both services. Nothing is cached: a plan granted a moment ago counts at once.
//!
//! A store this emulator cannot read at all — an engine that is not the in-memory one —
//! is EVIDENCE OF NOTHING and so refuses nothing, the convention every cross-service read
//! of this crate follows.

use crate::ops::types::ValidationExceptionField;

use ::service_com_amazonaws_support_awssupport_20130415::state as support;

/// The clause Amazon EVS states for an account that holds no Premium Support
/// subscription. Fixed text in full: it names no member of the request and interpolates
/// nothing from it, because what it reports is a fact about the account.
pub const ENTITLEMENT_CLAUSE: &str = "You must be subscribed to AWS Premium Support";

/// Whether the account holds a Premium Support subscription, as the Support store states
/// it. `true` for a store this emulator cannot read, which is evidence of nothing.
pub fn account_subscribed<Eng: Send + Sync + 'static>(engine: &Eng) -> bool {
    match support::service_state(engine) {
        Some(state) => state.lock().support_api_allowed(),
        None => true,
    }
}

/// The failure, if any, of the precondition that the account holds a Premium Support
/// subscription — the FIRST clause of the refusal a `CreateEnvironment` request from an
/// unsubscribed account draws, ahead of every other check that request is judged on.
///
/// The entry carries no field name: the clause names no member of the request, so there
/// is no member to group it under, and the recorded entries carry none either.
/// `ValidationExceptionField$name` is `@required` in the model, so the member is stated
/// empty rather than omitted.
pub fn entitlement_failure<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<ValidationExceptionField> {
    (!account_subscribed(engine)).then(|| super::field_failure_stated("", ENTITLEMENT_CLAUSE))
}
