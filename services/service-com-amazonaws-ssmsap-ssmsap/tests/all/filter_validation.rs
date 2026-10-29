// SPDX-License-Identifier: CC0-1.0

//! The `Filters` validation every filtered listing of this service shares, driven over
//! the wire through one operation per listing.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary.
//!
//! RULE under test: ONE rule decides whether a `Filter` is usable, whatever is being
//! listed — a filter is accepted when its `Operator` is one the listing offers and its
//! `Name` is one of the attributes that listing reports, and refused otherwise with the
//! operator checked before the name and the catalogue the listing could use named in the
//! refusal. The listings differ only in that catalogue, so a name one listing filters by
//! is refused by another that does not, and the wording of the refusal is the same for
//! all of them.
//!
//! The rows span what the rule generalises over: all four filtered listings
//! (`ListApplications`, `ListConfigurationCheckOperations`, `ListOperationEvents`,
//! `ListOperations`), every
//! filter name each one filters by crossed with every operator it offers, names drawn
//! from each OTHER listing's catalogue, a name in the wrong case (filter names are
//! case-sensitive), a name no shape of this service carries, a modelled operator a
//! listing does not offer, and an operator outside the model's `FilterOperator` enum.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{Filter, FILTER_OPERATOR_VALUES};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    validate_filters, FilterOperators, CONFIGURATION_CHECK_FILTER_NAMES, FILTER_NAMES,
    FILTER_OPERATOR_EQUALS, OPERATION_EVENT_FILTER_NAMES, OPERATION_FILTER_NAMES,
};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The application every listing here is scoped to.
///
/// `pub(crate)` so the suite that drives the shared length bounds over the same
/// listings (`tests/all/filter_length_bounds.rs`) filters by the id this registers.
pub(crate) const APPLICATION: &str = "HDB-FILTER-RULE";

/// A `Filter$Value` inside the modelled length bound; what a filter selects is another
/// suite's rule, so every row here carries the same value.
const VALUE: &str = "alpha";

/// An operator outside the model's `FilterOperator` enum
/// (`smithy_query(query="shape", shape="FilterOperator")`: Equals,
/// GreaterThanOrEquals, LessThanOrEquals).
const UNMODELLED_OPERATOR: &str = "NotEquals";

/// A `Filter$Name` no shape of this service carries.
const UNKNOWN_NAME: &str = "NotAMember";

/// One filtered listing: where a request to it goes, what it is scoped to, and which
/// filters it offers.
pub(crate) struct FilteredListing {
    pub(crate) label: &'static str,
    pub(crate) path: &'static str,
    /// The `Name`s it filters by, read off this crate's own catalogue for it rather than
    /// written out here, so a name added to either catalogue is exercised instead of
    /// going untested.
    pub(crate) names: &'static [&'static str],
    /// The operators it offers.
    operators: FilterOperators<'static>,
    /// The members that say WHAT is being listed, carried by every request to it.
    scope: Value,
}

impl FilteredListing {
    /// A request to this listing carrying `filters`.
    pub(crate) fn body(&self, filters: Vec<Value>) -> Value {
        let mut body = self.scope.clone();
        body.as_object_mut()
            .expect("the scope is a JSON object")
            .insert("Filters".to_string(), json!(filters));
        body
    }

    /// The operators this listing accepts.
    fn offered(&self) -> Vec<&'static str> {
        match self.operators {
            FilterOperators::Modelled => FILTER_OPERATOR_VALUES.to_vec(),
            FilterOperators::Only(only) => vec![only],
        }
    }

    /// The modelled operators this listing does NOT offer: empty for a listing that
    /// takes every one the model declares.
    fn not_offered(&self) -> Vec<&'static str> {
        FILTER_OPERATOR_VALUES
            .iter()
            .copied()
            .filter(|operator| !self.offered().contains(operator))
            .collect()
    }

    /// How this listing words a refusal of an operator it does not offer.
    fn operator_refusal(&self, operator: &str) -> String {
        match self.operators {
            FilterOperators::Modelled => format!(
                "Filter Operator must be one of {}, but was {operator}",
                FILTER_OPERATOR_VALUES.join(", ")
            ),
            FilterOperators::Only(only) => {
                format!("Filter Operator must be {only}, but was {operator}")
            }
        }
    }

    /// How this listing words a refusal of a name it does not filter by.
    pub(crate) fn name_refusal(&self, name: &str) -> String {
        format!(
            "Filter Name must be one of {}, but was {name}",
            self.names.join(", ")
        )
    }

    /// The names this listing must refuse: every name of another listing's catalogue
    /// that is not in its own, its own first name in the wrong case (filter names are
    /// case-sensitive, as `Filter$Name`'s documentation states), and a name no shape
    /// carries.
    fn refused_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for catalogue in [
            FILTER_NAMES,
            CONFIGURATION_CHECK_FILTER_NAMES,
            OPERATION_EVENT_FILTER_NAMES,
            OPERATION_FILTER_NAMES,
        ] {
            for name in catalogue {
                if !self.names.contains(name) && !names.iter().any(|held| held == name) {
                    names.push((*name).to_string());
                }
            }
        }
        if let Some(first) = self.names.first() {
            names.push(first.to_uppercase());
        }
        names.push(UNKNOWN_NAME.to_string());
        names
    }
}

/// One filter, as a request carries it.
fn filter(name: &str, operator: &str) -> Value {
    json!({"Name": name, "Value": VALUE, "Operator": operator})
}

/// Every filtered listing of this service, scoped to records this account and Region
/// holds: an application registered here, and the operation its registration filed.
pub(crate) async fn listings(emulator: &Emulator) -> Vec<FilteredListing> {
    let reply = emulator
        .send(
            "/register-application",
            valid_registration(APPLICATION, "HANA"),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from RegisterApplication, got {reply:?}"
    );
    let operation_id = reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !operation_id.is_empty(),
        "the registration must report the operation it filed, got {reply:?}"
    );
    settle().await;

    vec![
        FilteredListing {
            label: "ListApplications",
            path: "/list-applications",
            names: FILTER_NAMES,
            operators: FilterOperators::Modelled,
            scope: json!({}),
        },
        FilteredListing {
            label: "ListConfigurationCheckOperations",
            path: "/list-configuration-check-operations",
            names: CONFIGURATION_CHECK_FILTER_NAMES,
            operators: FilterOperators::Modelled,
            scope: json!({"ApplicationId": APPLICATION}),
        },
        FilteredListing {
            label: "ListOperationEvents",
            path: "/list-operation-events",
            names: OPERATION_EVENT_FILTER_NAMES,
            operators: FilterOperators::Only(FILTER_OPERATOR_EQUALS),
            scope: json!({"OperationId": operation_id}),
        },
        FilteredListing {
            label: "ListOperations",
            path: "/list-operations",
            names: OPERATION_FILTER_NAMES,
            operators: FilterOperators::Modelled,
            scope: json!({"ApplicationId": APPLICATION}),
        },
    ]
}

/// RULE: every filtered listing serves a filter whose name is one it reports and whose
/// operator is one it offers — one rule, applied with that listing's own catalogues.
#[tokio::test]
async fn every_listing_serves_each_name_and_operator_it_offers() {
    let emulator = Emulator::new();

    for listing in listings(&emulator).await {
        for name in listing.names {
            for operator in listing.offered() {
                let case = format!("{} {name} {operator}", listing.label);
                let reply = emulator
                    .send(listing.path, listing.body(vec![filter(name, operator)]))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected 200, got {}: {:?}",
                    reply.status, reply.body
                );
            }
        }
    }
}

/// RULE: a name a listing does not filter by is refused as `ValidationException`
/// (HTTP 400) naming the catalogue that listing could have used — so a name another
/// listing filters by is refused here, and the wording is the same for all of them.
#[tokio::test]
async fn every_listing_refuses_a_name_it_does_not_filter_by() {
    let emulator = Emulator::new();

    for listing in listings(&emulator).await {
        for name in listing.refused_names() {
            let case = format!("{} {name}", listing.label);
            let reply = emulator
                .send(
                    listing.path,
                    listing.body(vec![filter(&name, FILTER_OPERATOR_EQUALS)]),
                )
                .await;
            assert_refused(&reply, &case, &listing.name_refusal(&name));
        }
    }
}

/// RULE: a listing refuses an operator it does not offer, naming the operator it does —
/// and the operator it refuses is decided per listing, so a modelled operator one
/// listing refuses is served by a listing that offers it.
#[tokio::test]
async fn every_listing_refuses_an_operator_it_does_not_offer() {
    let emulator = Emulator::new();
    let listings = listings(&emulator).await;

    for listing in &listings {
        let name = listing.names.first().expect("a listing filters by a name");
        for operator in listing.not_offered() {
            let case = format!("{} {operator}", listing.label);
            let reply = emulator
                .send(listing.path, listing.body(vec![filter(name, operator)]))
                .await;
            assert_refused(&reply, &case, &listing.operator_refusal(operator));
            // The same operator is one ANOTHER listing offers, so the refusal is about
            // this listing's catalogue rather than about the operator itself.
            let elsewhere = listings
                .iter()
                .find(|other| other.offered().contains(&operator))
                .unwrap_or_else(|| panic!("case {case:?}: no listing offers {operator:?}"));
            let name = elsewhere
                .names
                .first()
                .expect("a listing filters by a name");
            let reply = emulator
                .send(elsewhere.path, elsewhere.body(vec![filter(name, operator)]))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: {} offers {operator:?}, got {}: {:?}",
                elsewhere.label, reply.status, reply.body
            );
        }
    }

    // And an operator outside the model's own enum is refused by every listing.
    for listing in &listings {
        let name = listing.names.first().expect("a listing filters by a name");
        let case = format!("{} {UNMODELLED_OPERATOR}", listing.label);
        let reply = emulator
            .send(
                listing.path,
                listing.body(vec![filter(name, UNMODELLED_OPERATOR)]),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }
}

/// RULE: the operator is checked BEFORE the name, so a filter wrong in both halves is
/// refused for its operator — the order of checks every listing shares, read off the one
/// validation each of them calls because the boundary refuses an unmodelled operator
/// before any handler runs.
#[test]
fn the_shared_validation_checks_the_operator_before_the_name() {
    // (label, the operators a listing offers, the names it filters by)
    let rows: [(&str, FilterOperators<'static>, &[&str]); 4] = [
        ("ListApplications", FilterOperators::Modelled, FILTER_NAMES),
        (
            "ListConfigurationCheckOperations",
            FilterOperators::Modelled,
            CONFIGURATION_CHECK_FILTER_NAMES,
        ),
        (
            "ListOperationEvents",
            FilterOperators::Only(FILTER_OPERATOR_EQUALS),
            OPERATION_EVENT_FILTER_NAMES,
        ),
        (
            "ListOperations",
            FilterOperators::Modelled,
            OPERATION_FILTER_NAMES,
        ),
    ];

    for (label, operators, names) in rows {
        let both_wrong = Filter {
            name: UNKNOWN_NAME.to_string(),
            value: VALUE.to_string(),
            operator: UNMODELLED_OPERATOR.to_string(),
        };
        let refusal = validate_filters(std::slice::from_ref(&both_wrong), operators, names)
            .expect_err("a filter wrong in both halves is refused");
        let actual = refusal.message.unwrap_or_default();
        assert!(
            actual.starts_with("Filter Operator must be "),
            "case {label:?}: the operator is checked first, got {actual:?}"
        );

        // Only the name wrong: the refusal names this listing's own catalogue.
        let bad_name = Filter {
            name: UNKNOWN_NAME.to_string(),
            value: VALUE.to_string(),
            operator: FILTER_OPERATOR_EQUALS.to_string(),
        };
        let refusal = validate_filters(std::slice::from_ref(&bad_name), operators, names)
            .expect_err("a name outside the catalogue is refused");
        let actual = refusal.message.unwrap_or_default();
        let expected = format!(
            "Filter Name must be one of {}, but was {UNKNOWN_NAME}",
            names.join(", ")
        );
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );

        // A filter this listing offers in both halves is accepted.
        for name in names {
            for operator in match operators {
                FilterOperators::Modelled => FILTER_OPERATOR_VALUES.to_vec(),
                FilterOperators::Only(only) => vec![only],
            } {
                let usable = Filter {
                    name: (*name).to_string(),
                    value: VALUE.to_string(),
                    operator: operator.to_string(),
                };
                assert!(
                    validate_filters(std::slice::from_ref(&usable), operators, names).is_ok(),
                    "case {label:?} {name:?}/{operator:?}: a usable filter is accepted"
                );
            }
        }
    }
}

/// A refusal the handler raised: 400, `ValidationException`, and the exact message the
/// shared validation words for it.
fn assert_refused(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected 400, got {}: {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let actual = message(reply);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}
