// SPDX-License-Identifier: CC0-1.0

//! The order this service states an enumerated type's values in when it refuses a
//! value outside them.
//!
//! A refusal that lists an enum's values states them in the order the SERVICE
//! publishes for that enum. That order is a fact about the served surface, not about
//! the checked-in model: the model's member ordering is how the shape was written
//! down, and for some enums the service lists the same values in another order. This
//! module names the published order once per enum it differs for, keyed by the value
//! SET rather than by a member or an operation, so every member targeting that enum —
//! whichever operation carried it, and whether the value was refused on the request
//! boundary or by a handler reading the member itself — states the one list.
//!
//! Split out of the state module root so this one concern lives in its own file; every
//! item is re-exported from [`crate::state`], so a caller still reaches it as
//! `crate::state::…`.

use smithy_server_runtime::{Violation, ViolationKind};

/// The published value order of every enumerated type of this service whose order
/// differs from the order its model shape declares its values in.
///
/// One row per enum, written as the whole list in published order, so a row both
/// identifies the enum (by its value set) and states the order. The rows:
///
/// * `PricingPlan` (`smithy_query(query="shape",shape="PricingPlan")` declares
///   `NONE, ESSENTIALS, PRO, ENTERPRISE`) — published as `ENTERPRISE, ESSENTIALS, PRO,
///   NONE`, the list SES states when it refuses a plan outside the set.
///
/// Every other enum of this service is stated in the order its model shape declares,
/// which is what an absent row means.
const PUBLISHED_ENUM_VALUE_ORDER: [&[&str]; 1] = [&["ENTERPRISE", "ESSENTIALS", "PRO", "NONE"]];

/// The order this service states the value set `allowed` in, or `None` when that is
/// the order `allowed` already carries — the model's own, which is what every enum
/// outside [`PUBLISHED_ENUM_VALUE_ORDER`] is stated in.
///
/// A row is matched on its value SET: same number of values, and each value of the
/// row present in `allowed`. The set identifies the enum, so a caller holding the
/// values in any order (a model-declared list, a sorted catalog) resolves to the same
/// row.
pub fn published_enum_value_order<S: AsRef<str>>(allowed: &[S]) -> Option<&'static [&'static str]> {
    PUBLISHED_ENUM_VALUE_ORDER
        .into_iter()
        .find(|published| {
            published.len() == allowed.len()
                && published
                    .iter()
                    .all(|value| allowed.iter().any(|held| held.as_ref() == *value))
        })
        .filter(|published| {
            // Nothing to restate when the caller already holds the published order.
            published
                .iter()
                .zip(allowed.iter())
                .any(|(published, held)| held.as_ref() != *published)
        })
}

/// `allowed` as this service states it: the published order when one differs from the
/// order the caller holds the values in, and that order otherwise.
pub fn published_enum_value_list<S: AsRef<str>>(allowed: &[S]) -> String {
    match published_enum_value_order(allowed) {
        Some(published) => published.join(", "),
        None => allowed
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// `violation` with the value set it names stated in the order this service publishes
/// for that enum, and any other violation handed back as it came.
///
/// The violation is REMAPPED rather than its rendered sentence rewritten: the clause,
/// its member path, the value it echoes or withholds and the requirement's wording all
/// stay the constraint walk's, and only the values that go into the list are reordered.
/// So a clause the walk words differently — a value set nested inside another
/// requirement, a new constraint class — renders as the walk words it, with the one
/// order this service states.
pub fn in_published_enum_value_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    let Some(published) = published_enum_value_order(declared) else {
        return violation.clone();
    };
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(published.iter().map(|value| value.to_string()).collect()),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value set no row names is stated in the order it was handed over.
    #[test]
    fn an_enum_outside_the_catalog_keeps_the_order_it_was_handed_over_in() {
        let allowed = ["PENDING", "SUCCESS", "FAILED"];
        assert!(
            published_enum_value_order(&allowed).is_none(),
            "case uncatalogued: expected no published order, got {:?}",
            published_enum_value_order(&allowed)
        );
        assert_eq!(
            published_enum_value_list(&allowed),
            "PENDING, SUCCESS, FAILED",
            "case uncatalogued: expected the order handed over, got {:?}",
            published_enum_value_list(&allowed)
        );
    }

    /// A catalogued set is stated in the published order whichever order the caller
    /// holds it in, and a remapped violation keeps everything but the values' order.
    #[test]
    fn a_catalogued_enum_is_stated_in_the_published_order() {
        for (case, allowed) in [
            ("model order", ["NONE", "ESSENTIALS", "PRO", "ENTERPRISE"]),
            (
                "sorted catalog",
                ["ENTERPRISE", "ESSENTIALS", "NONE", "PRO"],
            ),
        ] {
            assert_eq!(
                published_enum_value_list(&allowed),
                "ENTERPRISE, ESSENTIALS, PRO, NONE",
                "case {case:?}: expected the published order, got {:?}",
                published_enum_value_list(&allowed)
            );
            let remapped = in_published_enum_value_order(&enum_violation(&allowed));
            assert_eq!(
                remapped.kind,
                ViolationKind::EnumValue(
                    ["ENTERPRISE", "ESSENTIALS", "PRO", "NONE"]
                        .map(str::to_string)
                        .to_vec()
                ),
                "case {case:?}: expected the values in the published order, got {:?}",
                remapped.kind
            );
            assert_eq!(
                (remapped.path.as_str(), remapped.value.clone(), remapped.sensitive),
                ("plan", Some("PLATINUM".to_string()), false),
                "case {case:?}: expected the clause's other facts untouched, got {remapped:?}"
            );
        }
    }

    /// A violation about anything but a value set is handed back as it came.
    #[test]
    fn a_violation_about_another_constraint_is_unchanged() {
        let violation = Violation {
            path: "plan".to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let remapped = in_published_enum_value_order(&violation);
        assert_eq!(
            (remapped.kind, remapped.path),
            (violation.kind.clone(), violation.path.clone()),
            "case other constraint: expected the violation unchanged"
        );
    }

    /// A violation naming a value set no row names keeps the walk's own order.
    #[test]
    fn an_uncatalogued_value_set_keeps_the_walks_order() {
        let allowed = ["PENDING", "SUCCESS", "FAILED"];
        let remapped = in_published_enum_value_order(&enum_violation(&allowed));
        assert_eq!(
            remapped.kind,
            ViolationKind::EnumValue(allowed.map(str::to_string).to_vec()),
            "case uncatalogued: expected the walk's own order, got {:?}",
            remapped.kind
        );
    }

    /// One violation of a member refused for a value outside `allowed`.
    fn enum_violation(allowed: &[&str]) -> Violation {
        Violation {
            path: "plan".to_string(),
            kind: ViolationKind::EnumValue(allowed.iter().map(|value| value.to_string()).collect()),
            value: Some("PLATINUM".to_string()),
            sensitive: false,
        }
    }
}
