// SPDX-License-Identifier: CC0-1.0

//! The order ACM states an enum's value set in when it refuses a value outside it.
//!
//! A constraint clause about an enum member states WHICH values the enum admits
//! (`Member must satisfy enum value set: [<values>]`). The values are the model's, but
//! the ORDER they are listed in is the service's own: it is neither the order the model
//! declares them in nor a sort of them, so it cannot be derived from the model and is
//! carried here, one row per enum ACM publishes an order for.
//!
//! An enum is recognised by its value SET rather than by the member that carried the
//! rejected value, so every member targeting one enum — of any operation, at any depth,
//! and whichever seam refused the request (the boundary walk or a handler) — states that
//! enum's set in the one order. An enum this service publishes no order for is stated in
//! the order it was presented in, which is the only order the emulator has a source for.

use std::collections::BTreeSet;

/// The order ACM states `CertificateStatus`'s value set in.
///
/// Holds exactly the values `crate::ops::types::CERTIFICATE_STATUS_VALUES` holds — the
/// model's `CertificateStatus` enum — and neither in that declaration order nor sorted:
/// this is the order ACM's own refusal of a status outside the set lists them in.
pub(crate) const CERTIFICATE_STATUS_VALUE_SET: [&str; 7] = [
    "VALIDATION_TIMED_OUT",
    "PENDING_VALIDATION",
    "EXPIRED",
    "INACTIVE",
    "ISSUED",
    "FAILED",
    "REVOKED",
];

/// The order ACM states `CertificateType`'s value set in, on the same footing as
/// [`CERTIFICATE_STATUS_VALUE_SET`]: the values of the model's `CertificateType` enum
/// (`crate::ops::types::CERTIFICATE_TYPE_VALUES`), in the order ACM's refusal lists
/// them.
pub(crate) const CERTIFICATE_TYPE_VALUE_SET: [&str; 3] = ["PRIVATE", "IMPORTED", "AMAZON_ISSUED"];

/// Every enum value set this service publishes an order for.
///
/// One row per enum whose stated order is known to differ from the order it reaches a
/// clause in; a value set absent from this table is stated as presented, because this
/// service has shown no other order for it.
const PUBLISHED_ENUM_VALUE_SETS: &[&[&str]] =
    &[&CERTIFICATE_STATUS_VALUE_SET, &CERTIFICATE_TYPE_VALUE_SET];

/// The order this service states `presented` in, when `presented` is the value set of an
/// enum whose order it publishes ([`PUBLISHED_ENUM_VALUE_SETS`]); `None` for any other
/// value set.
///
/// Matched on the SET of values, so the member, the operation and the depth the value
/// sat at do not enter into it: one enum, one stated order.
pub(crate) fn published_order(presented: &[&str]) -> Option<&'static [&'static str]> {
    let presented: BTreeSet<&str> = presented.iter().copied().collect();
    PUBLISHED_ENUM_VALUE_SETS
        .iter()
        .copied()
        .find(|published| {
            published.len() == presented.len()
                && published.iter().all(|value| presented.contains(value))
        })
}
