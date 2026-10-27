// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-constraint refusals: how the structured
//! [`Refusal`]s the framework's boundary walk produces become the error code and the
//! `message` an awsQuery caller is refused with.
//!
//! [`crate::service`] owns the SEAM (`Service::override_refusal`) and this module owns the
//! sentence, so the clause rules live in one place rather than in the seam that applies
//! them. WHAT is enforced is always the model's: every violation handed here comes from
//! `aws_smithy_schema_server::constraints`, which reads the `@required`, `@length`,
//! `@range`, `@pattern`, `@enum` and `@uniqueItems` traits codegen captured off the model.
//! Only the WORDS are chosen here.
//!
//! The rules, each holding for every constrained member of every operation rather than for
//! the members one report happened to cite:
//!
//! 1. one error CODE for every structural refusal of this query API, whichever layer found
//!    it ([`VALIDATION_ERROR`]) — an absent required member is found while READING the
//!    request and a bad value while WALKING the one that was read, but to a caller they
//!    are one class of refusal and a client branches on one code for both. The framework's
//!    own default splits them (`MalformedInput` for the read, `ValidationError` for the
//!    walk), which is what this rule replaces;
//! 2. the envelope is AWS's `N validation error(s) detected: ` over one clause per
//!    offending member, joined with `; ` in the order the walk found them — the
//!    framework's own, so it cannot drift;
//! 3. a clause states the value the member held, its dotted path and the requirement it
//!    failed — EXCEPT for the two classes whose clause states the position and the
//!    REQUIREMENT alone ([`states_the_value`]): an ABSENT member, which held no value to
//!    report, and a value outside an enum's admitted set, where that set is the whole of
//!    what a caller needs in order to resubmit. Both are decided by the violation's CLASS,
//!    so each holds for every member of every operation carrying that constraint;
//! 4. an enum clause states the admitted VALUE SET and the position —
//!    `Value at '<path>' failed to satisfy constraint: Member must satisfy enum value set:
//!    [ … ]`. The set's MEMBERSHIP is the model's; the ORDER it is listed in is this
//!    service's own listing of that enum's values ([`VALUE_LISTINGS`], [`listed`]), which
//!    no order the model carries reproduces. The order is a property of the ENUM, not of
//!    the member or the operation reaching it, so one enum states one ordered set wherever
//!    it is reached.
//!
//! A violation class this module has no wording for keeps the FRAMEWORK's own clause for
//! that one violation ([`ViolationKind`] is `#[non_exhaustive]`, so a class can be added
//! upstream), which leaves the rest of the refusal worded here.
//!
//! An authentication refusal is not a structural one and keeps the framework's 403 and
//! wording.

use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{Refusal, Refused};

/// The error code this service's query API answers EVERY structural refusal with (rule 1).
///
/// It is also the code the framework's query renderer picks for a value that failed a
/// declared bound, so the rule changes only the other half — a member the request left
/// out, which the framework answers `MalformedInput` for.
const VALIDATION_ERROR: &str = "ValidationError";

/// The order this service states an enum's admitted values in, for the enums it is
/// observed to publish a listing for.
///
/// An enum clause states a MEMBERSHIP and an ORDER. The membership is the model's, always.
/// The order is not: this service lists `IdentityType` as `Domain, EmailAddress` while the
/// model declares it `EmailAddress, Domain`
/// (`smithy_query(query="shape",shape="IdentityType")`). It is service metadata the public
/// model does not carry, and the only source for it is what this service's refusals are
/// recorded stating — so a listing is recorded per enum HERE, beside the one renderer
/// every enum-constrained member of every operation goes through, rather than in an
/// operation handler.
///
/// A listing is identified by the SET it orders — which is what a violation carries, the
/// clause having no shape id — and is applied ONLY to an enum whose admitted set it is a
/// permutation of ([`orders_the_same_set`]). A listing can therefore neither add a value
/// this model does not declare nor drop one it does, and an enum whose membership changes
/// in the model stops matching a stale listing and states the model's order instead. An
/// enum with no recorded listing keeps the order the member schema carries, which is the
/// model's.
///
/// What a membership cannot identify is one enum among several declared over the SAME
/// values: such a listing would order both. No two listings here may therefore order the
/// same set, which `no_two_listings_order_the_same_set` holds them to.
const VALUE_LISTINGS: &[&[&str]] = &[
    // `IdentityType` (`ListIdentities.IdentityType`), declared `EmailAddress, Domain`.
    &["Domain", "EmailAddress"],
];

/// `values` in the order this service lists them, or in the order they arrived — the
/// model's, off the member schema — when this service publishes no listing for that enum.
fn listed(values: &[String]) -> Vec<String> {
    match VALUE_LISTINGS
        .iter()
        .find(|listing| orders_the_same_set(listing, values))
    {
        Some(listing) => listing.iter().map(|value| (*value).to_string()).collect(),
        None => values.to_vec(),
    }
}

/// Whether two value lists hold the same values, in whatever order each holds them.
fn orders_the_same_set(one: &[&str], other: &[String]) -> bool {
    if one.len() != other.len() {
        return false;
    }
    let mut one: Vec<&str> = one.to_vec();
    let mut other: Vec<&str> = other.iter().map(String::as_str).collect();
    one.sort_unstable();
    other.sort_unstable();
    one == other
}

/// Whether a clause of this violation's class states the value the member held (rule 3).
///
/// The two exceptions are classes, not members: an ABSENT member held nothing to report,
/// and an enum rejection's clause carries the admitted set, which is what a caller acts
/// on.
fn states_the_value(kind: &ViolationKind) -> bool {
    !matches!(
        kind,
        ViolationKind::MissingRequiredMember | ViolationKind::EnumValue(_)
    )
}

/// `violation` with the enum ORDER this service lists (rule 4) and nothing else changed.
fn restated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::EnumValue(values) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::EnumValue(listed(values)),
            value: violation.value.clone(),
        },
        _ => violation.clone(),
    }
}

/// The clause a violation of a valueless class states: the framework's own with the
/// `Value null ` / `Value '<submitted>' ` half reduced to `Value `, so the position and
/// the requirement stay exactly the framework's. `None` when the class states its value.
fn valueless_clause(violation: &Violation) -> Option<String> {
    if states_the_value(&violation.kind) {
        return None;
    }
    let stated = violation.message();
    let value_half = match violation.value.as_deref() {
        Some(submitted) => format!("Value '{submitted}' "),
        // An absent member: the framework writes the literal `null` where a value would go.
        None => "Value null ".to_string(),
    };
    let requirement_half = stated.strip_prefix(&value_half)?;
    Some(format!("Value {requirement_half}"))
}

/// State `refusal` the way this service states it, or `None` to keep the framework's own
/// wording — the whole `Service::override_refusal` body for this service.
///
/// The code travels with the message, always: a client branches on the code, so a message
/// without its matching code is worse than leaving both alone.
pub fn refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        // Not a structural refusal: an unauthenticated caller keeps the framework's 403
        // code and wording.
        return None;
    };
    let restated: Vec<Violation> = violations.iter().map(restated).collect();
    // The envelope and every clause are the framework's; the valueless classes then drop
    // their value half, which the framework has no way to leave out (a clause carrying no
    // value reads `Value null at …`, which is how it states an ABSENT member).
    let mut message = envelope(&restated)?;
    for violation in &restated {
        if let Some(valueless) = valueless_clause(violation) {
            message = message.replace(&violation.message(), &valueless);
        }
    }
    Some(Refused {
        code: VALIDATION_ERROR.to_string(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::{orders_the_same_set, refusal, VALUE_LISTINGS};
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use smithy_server_runtime::Refusal;

    /// An absent member is stated with the position and the requirement, under the one
    /// code every structural refusal of this API carries.
    #[test]
    fn an_absent_member_is_stated_without_a_value() {
        let stated = refusal(&Refusal::Invalid(vec![Violation {
            path: "destination".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        }]))
        .expect("a structural refusal is stated");
        assert_eq!(
            (stated.code.as_str(), stated.message.as_str()),
            (
                "ValidationError",
                "1 validation error detected: Value at 'destination' failed to satisfy \
                 constraint: Member must not be null"
            ),
            "case an absent required member: got {stated:?}"
        );
    }

    /// An enum rejection states the admitted set in this service's listed order and does
    /// not echo the value it refused.
    #[test]
    fn an_enum_rejection_states_the_listed_set_and_no_value() {
        let stated = refusal(&Refusal::Invalid(vec![Violation {
            path: "identityType".to_string(),
            kind: ViolationKind::EnumValue(vec![
                "EmailAddress".to_string(),
                "Domain".to_string(),
            ]),
            value: Some("Bogus".to_string()),
        }]))
        .expect("a structural refusal is stated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'identityType' failed to satisfy \
             constraint: Member must satisfy enum value set: [Domain, EmailAddress]",
            "case a value outside an enum's set: got {stated:?}"
        );
    }

    /// A listing reaches an enum by the SET it orders, so two listings over one set would
    /// each claim the other's enum: the table may hold at most one listing per membership.
    #[test]
    fn no_two_listings_order_the_same_set() {
        for (index, listing) in VALUE_LISTINGS.iter().enumerate() {
            let owned: Vec<String> = listing.iter().map(|value| (*value).to_string()).collect();
            for other in VALUE_LISTINGS.iter().skip(index + 1) {
                assert!(
                    !orders_the_same_set(other, &owned),
                    "case {listing:?}: another listing orders the same set ({other:?}), so \
                     neither can identify its own enum"
                );
            }
        }
    }

    /// Every other class keeps the framework's clause, value and all.
    #[test]
    fn another_class_keeps_the_frameworks_clause() {
        let stated = refusal(&Refusal::Invalid(vec![Violation {
            path: "maxItems".to_string(),
            kind: ViolationKind::MaxValue("1000".to_string()),
            value: Some("1001".to_string()),
        }]))
        .expect("a structural refusal is stated");
        assert_eq!(
            stated.message,
            "1 validation error detected: Value '1001' at 'maxItems' failed to satisfy \
             constraint: Member must have value less than or equal to 1000",
            "case a value past a declared bound: got {stated:?}"
        );
    }
}
