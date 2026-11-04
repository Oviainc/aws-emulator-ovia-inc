// SPDX-License-Identifier: CC0-1.0

//! The sentences Service Quotas states its refusals in: ONE wording per FACT, shared by
//! every operation that can state that fact.
//!
//! Each sentence here is the service's own, fixed for its error class: it is the same
//! string whatever the request carried, so nothing of the request — no `ServiceCode`, no
//! `QuotaCode`, no `RequestId`, no `DesiredValue` — is interpolated into it. The one
//! sentence with a variable part is the refusal of a quota value below the quota's AWS
//! default, which names that default; it is read out of this service's own catalog, not out
//! of the request.
//!
//! Which fact a refusal states is a property of what the request named and what of it
//! was not there:
//!
//! * a call that scopes itself to a `ServiceCode` the catalog does not publish — a
//!   listing keyed by one, or an Automatic Management exclusion keyed by one — is
//!   answered with [`SERVICE_NOT_AVAILABLE_MESSAGE`]: an unrecognised service code is a
//!   service that is not integrated with Service Quotas at the endpoint the call was
//!   made at, which the caller can act on by choosing another Region or another service;
//! * a call that resolves ONE quota by its `(ServiceCode, QuotaCode)` pair is answered
//!   with [`QUOTA_AND_SERVICE_ABSENT_MESSAGE`] whichever half of the pair is the one that
//!   does not exist — one sentence for the failed lookup, naming both halves;
//! * a call that resolves one quota increase request by its `RequestId` is answered with
//!   [`NO_SUCH_QUOTA_REQUEST_MESSAGE`];
//! * a quota value that is not an increase is answered by [`not_an_increase_message`],
//!   which puts the two bounds in the order the service checks them.
//!
//! The sentence for a value this service's own application layer will not accept lives
//! beside the validator that states it, as [`super::ILLEGAL_VALUE_MESSAGE`].

/// How Service Quotas states that the `ServiceCode` a call is scoped to is not one it
/// serves quotas for at the endpoint the call was made at, for the modelled
/// `NoSuchResourceException`.
///
/// Fixed text, down to the trailing space the service sends it with: the code the caller
/// named appears nowhere in it.
pub const SERVICE_NOT_AVAILABLE_MESSAGE: &str = "This service is not available in the \
                                                 current Region. Choose a different \
                                                 Region or a different service. ";

/// How Service Quotas states that a `(ServiceCode, QuotaCode)` pair a call named resolves
/// to no quota, for the modelled `NoSuchResourceException`.
///
/// ONE sentence for that lookup however it failed — a service code naming no service, a
/// code the service it names holds no quota under, or both — and wholly fixed text: the
/// sentence names the quota AND the service because it tells the caller that either may be
/// the thing that does not exist, and neither code the caller sent appears in it.
pub const QUOTA_AND_SERVICE_ABSENT_MESSAGE: &str =
    "The request failed because the specified quota and service do not exist.";

/// How Service Quotas states that no quota increase request answers to the `RequestId` a
/// call named, for the modelled `NoSuchResourceException`.
///
/// Fixed text: the id the caller supplied is not echoed back into the refusal.
pub const NO_SUCH_QUOTA_REQUEST_MESSAGE: &str = "The request failed because the specified \
                                                 requestId to increase the service quota \
                                                 does not exist.";

/// The fixed half of the refusal of a quota value below the quota's AWS default: the
/// sentence ends with that default value.
const BELOW_DEFAULT_CLAUSE: &str =
    "You must provide a quota value greater than the default quota value of ";

/// How Service Quotas refuses a quota value that is not above the value already IN FORCE
/// for the quota, where that value clears the quota's AWS default, for the modelled
/// `IllegalArgumentException`.
///
/// Wholly fixed text: this bound names no value at all, where the default bound names the
/// default ([`BELOW_DEFAULT_CLAUSE`]).
pub const NOT_ABOVE_CURRENT_VALUE_MESSAGE: &str =
    "You must provide a quota value greater than the current quota value";

/// How Service Quotas refuses a `DesiredValue` that is not an increase, with the two bounds
/// in the order the service checks them: the quota's AWS DEFAULT first — a value below it
/// is refused naming that default — and only then the value in force, which is refused in
/// the fixed sentence that names no value.
///
/// Both bounds are properties of the VALUE the request carried against this service's own
/// catalog and state, so which sentence is stated follows from where `desired_value` falls
/// and not from what state the quota happens to be in. Asked here rather than at each
/// caller, so the write that asks for an increase and the template entry that carries one
/// order the bounds the same way.
pub fn not_an_increase_message(desired_value: f64, default_value: f64) -> String {
    if desired_value < default_value {
        below_default_value_message(default_value)
    } else {
        NOT_ABOVE_CURRENT_VALUE_MESSAGE.to_string()
    }
}

/// How Service Quotas refuses a quota value below the quota's AWS DEFAULT value, for the
/// modelled `IllegalArgumentException`.
///
/// The variable part is the quota's own default value, read from this service's catalog
/// and rendered as a decimal ([`quota_value_text`]); nothing of the request appears.
pub fn below_default_value_message(default_value: f64) -> String {
    format!(
        "{BELOW_DEFAULT_CLAUSE}{}",
        quota_value_text(default_value)
    )
}

/// A `QuotaValue` as a refusal renders one: a decimal carrying at least one fractional
/// digit, so a whole value reads as `5.0` rather than `5`.
///
/// Built from the value's own shortest decimal rendering, with the fractional part added
/// when it has none, so a value that is not whole keeps every digit it needs. A
/// `QuotaValue` is `@range 0..10000000000`, so every value that reaches this is finite.
pub fn quota_value_text(value: f64) -> String {
    let rendered = format!("{value}");
    if rendered.contains('.') {
        rendered
    } else {
        format!("{rendered}.0")
    }
}

/// How an Automatic Management write states that its `ExclusionList` names something this
/// service publishes no quota for.
///
/// An entry is keyed by a `ServiceCode` and names `QuotaCode`s under it, so the key that
/// names no service states the service fact (the entry need not name a quota at all) and a
/// code under a published service states the failed pair lookup.
pub fn exclusion_refusal_message(refusal: &crate::state::ExclusionRefusal) -> &'static str {
    match refusal {
        crate::state::ExclusionRefusal::UnknownService { .. } => SERVICE_NOT_AVAILABLE_MESSAGE,
        crate::state::ExclusionRefusal::UnknownQuota { .. } => QUOTA_AND_SERVICE_ABSENT_MESSAGE,
    }
}
