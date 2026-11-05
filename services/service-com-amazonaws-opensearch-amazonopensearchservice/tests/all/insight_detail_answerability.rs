// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a detail read taken against a store that holds NO insight.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], the detail request from [`super::insight_details`], the
//! listing from [`super::insights`] and the constraint report from
//! [`super::constraint_refusal_messages`]; this suite adds no second copy of any of them.
//!
//! The rule under test: a detail read is an informational REPORT that is answerable for
//! every entity this service resolves — an id that entity holds no insight under is
//! answered 200 with the `@required` `Fields` list carrying at least one field, each with
//! a `Name`, a modelled `Type` and a `Value`, rather than refused as a missing resource —
//! and what it answers there carries no part of the request, so the view is the same
//! whichever resolvable entity and whichever well-formed id a read names. Nothing else is
//! relaxed by that: an entity whose type is outside `InsightEntityType`, an `Account`
//! entity naming another account and a `DomainName` this account holds no domain under are
//! still refused, and a read writes nothing.
//!
//! No insight is seeded anywhere, which is what tells this suite apart from
//! [`super::insight_details`]: an insight is RAISED by the service observing a domain, so
//! a client meets a store that holds none, and that is the state every detail read of a
//! real client has to be answered in.
//!
//! So the assertions span a SET of inputs rather than one call: four entities — the account
//! named both ways the model admits, a domain polled to `Created && !Processing` and one
//! still provisioning — against four well-formed ids, in each of the three ways a request
//! can answer `ShowHtmlContent`; then five entity types outside the modelled set across
//! both operations that take an `InsightEntity`, and three entities this account holds
//! nothing for.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    INSIGHT_ENTITY_TYPE_VALUES, INSIGHT_FIELD_TYPE_VALUES,
};

use super::constraint_refusal_messages::{clause, message, report, value_set};
use super::domain_lifecycle::{
    create_baseline, Answer, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::insight_details::{describe_insight, domain_entity, guid};
use super::insights::{list, refused};

/// The domain a case reads against once the clock owes it its terminal status — the
/// precondition a client polls for before it reads insight detail.
const READY_DOMAIN: &str = "insight-answerable-ready";

/// A domain still inside its provisioning span, so readiness is not what makes a detail
/// read answerable.
const PROVISIONING_DOMAIN: &str = "insight-answerable-new";

/// The ids every case below reads.
///
/// All four are well-formed `GUID`s — `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$` — and no
/// insight was ever raised under any of them. The rows differ along the shape of the id:
/// every group at its lowest digit, every group at its highest, hex digits in both cases
/// (the pattern admits either), and one of the ordinary form this crate builds ids in.
fn unheld_ids() -> Vec<String> {
    vec![
        "00000000-0000-0000-0000-000000000000".to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        "A1b2C3d4-E5f6-7a8b-9C0d-1e2F3a4B5c6D".to_string(),
        guid("0000000000ff"),
    ]
}

/// Whether a `DescribeDomain` answer reports the `Created && !Processing` pair a readiness
/// poll waits for.
fn is_ready(answer: &Answer) -> bool {
    let status = &answer.body["DomainStatus"];
    status["Created"].as_bool() == Some(true) && status["Processing"].as_bool() == Some(false)
}

/// The `Fields` of an answered detail read, asserted to be the view the rule states: a
/// non-empty list, every element carrying the three `@required` members of `InsightField`
/// with a `Type` inside the catalog the model declares.
fn answered_fields(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.body["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Fields list, got {:?}", answer.body))
        .clone();
    assert!(
        !reported.is_empty(),
        "case {case}: expected at least one informational field, got {:?}",
        answer.body
    );
    for field in &reported {
        let name = field["Name"].as_str().unwrap_or_default();
        assert!(
            !name.is_empty(),
            "case {case}: expected a non-empty Name on every field, got {field:?}"
        );
        assert!(
            field["Value"].is_string(),
            "case {case}: expected a string Value on every field, got {field:?}"
        );
        // The type catalog is the model's own enum, as generated into `ops::types`.
        let field_type = field["Type"].as_str().unwrap_or_default();
        assert!(
            INSIGHT_FIELD_TYPE_VALUES.contains(&field_type),
            "case {case}: expected a modelled field type, got {field:?}"
        );
    }
    reported
}

/// Whether the store holds no insight at all: neither for the account nor on any domain.
///
/// Read the way a handler reaches the store, so a read that raised an insight to answer
/// with would be visible here even if the wire answer looked right.
fn holds_no_insight(emulator: &Emulator) -> bool {
    let store = emulator.store();
    let guard = store.lock();
    guard.account_insights.is_empty()
        && guard
            .domains
            .values()
            .all(|domain| domain.insights.is_empty())
}

/// The `Insights` of a listing for `entity`, asserted to be the empty set this emulator
/// holds for every entity.
async fn empty_listing(emulator: &Emulator, entity: &Value, case: &str) {
    let answer = list(emulator, json!({"Entity": entity})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListInsights, got {} with {:?}",
        answer.status, answer.body
    );
    let listed = answer.body["Insights"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        listed, 0,
        "case {case}: expected an empty listing, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_detail_read_is_answerable_for_every_entity_this_service_resolves() {
    let emulator = Emulator::new();
    create_baseline(&emulator, READY_DOMAIN).await;
    // The precondition a client polls for: the domain has reached the status the clock
    // owes it.
    tokio::time::sleep(SETTLE_WAIT).await;
    let ready = emulator.describe(READY_DOMAIN).await;
    assert!(
        is_ready(&ready),
        "case {READY_DOMAIN}: expected Created && !Processing once the span has passed, got {:?}",
        ready.body["DomainStatus"]
    );
    // And one domain inside its provisioning span, so what makes the read answerable is
    // that the entity resolves and not that it has settled.
    create_baseline(&emulator, PROVISIONING_DOMAIN).await;
    let provisioning = emulator.describe(PROVISIONING_DOMAIN).await;
    assert!(
        !is_ready(&provisioning),
        "case {PROVISIONING_DOMAIN}: expected the transient state inside the span, got {:?}",
        provisioning.body["DomainStatus"]
    );

    // Every entity this service resolves: the account named both ways a caller can name it
    // — `Account`'s `Value` is optional, and the only account served is the one the request
    // was made for — and a `DomainName` entity for each of the two domains.
    let entities = [
        json!({"Type": "Account"}),
        json!({"Type": "Account", "Value": engine::emulator_account_id()}),
        domain_entity(READY_DOMAIN),
        domain_entity(PROVISIONING_DOMAIN),
    ];

    let mut views: Vec<(String, Vec<Value>)> = Vec::new();
    for entity in &entities {
        assert!(
            INSIGHT_ENTITY_TYPE_VALUES.contains(&entity["Type"].as_str().unwrap_or_default()),
            "case {entity:?}: the row must name a modelled entity type"
        );
        // Nothing was ever raised for this entity, which is what every row below reads
        // against — and the listing says so on the wire, not just in the store.
        empty_listing(&emulator, entity, &format!("ListInsights({entity:?})")).await;

        for unheld in unheld_ids() {
            // The three ways a request can answer `ShowHtmlContent`: a read with no
            // stored field to render answers the same either way.
            for extra in [
                json!({}),
                json!({"ShowHtmlContent": false}),
                json!({"ShowHtmlContent": true}),
            ] {
                let case =
                    format!("DescribeInsightDetails({entity:?}, {unheld:?}, {extra:?})");
                let answer =
                    describe_insight(&emulator, entity.clone(), &unheld, extra.clone()).await;
                views.push((case.clone(), answered_fields(&answer, &case)));

                // A report: the same request answers the same thing again.
                let again =
                    describe_insight(&emulator, entity.clone(), &unheld, extra).await;
                assert_eq!(
                    again.body, answer.body,
                    "case {case}(twice): expected the same view, got {:?}",
                    again.body
                );
            }
        }

        // Reading detail raised no insight for the entity, so its listing is what it was.
        empty_listing(
            &emulator,
            entity,
            &format!("ListInsights({entity:?}) after the reads"),
        )
        .await;
    }

    // The view the service answers with where it holds no detail to report carries no part
    // of the request: every entity and every id above is answered with the same fields, so
    // nothing of the address leaks into it.
    let (first_case, first) = views
        .first()
        .expect("the table drives at least one detail read")
        .clone();
    for (case, reported) in &views {
        assert_eq!(
            reported, &first,
            "case {case}: expected the same informational view as {first_case}, got {reported:?}"
        );
    }

    // And nothing was written: a detail read reports, it does not raise an insight to
    // report on.
    assert!(
        holds_no_insight(&emulator),
        "case the whole table: expected a detail read to store no insight"
    );
}

#[tokio::test]
async fn an_entity_type_outside_the_modelled_set_is_refused_with_its_value_set() {
    let emulator = Emulator::new();
    create_baseline(&emulator, READY_DOMAIN).await;
    let unheld = unheld_ids();
    let insight_id = unheld
        .first()
        .expect("the id table is not empty")
        .to_owned();

    // Rows differ along the shape of the type: a value of the right kind in the wrong
    // case, the singular of a modelled value, a value of the OTHER entity enum of this
    // service, a type of no entity at all, and no type at all.
    for not_a_type in ["account", "DOMAINNAME", "Domain", "Cluster", ""] {
        assert!(
            !INSIGHT_ENTITY_TYPE_VALUES.contains(&not_a_type),
            "case {not_a_type:?}: the row must name a type outside the modelled set"
        );
        let entity = json!({"Type": not_a_type, "Value": READY_DOMAIN});
        // The clause is the member's own: the value echoed at the path the member is
        // carried under, and the value set the model declares for it.
        let expected = report(&[clause(
            not_a_type,
            "entity.type",
            &value_set(INSIGHT_ENTITY_TYPE_VALUES),
        )]);

        // Both operations that take an `InsightEntity` resolve the entity the same way, so
        // both state the same clause for a type outside the set.
        for (case, answer) in [
            (
                format!("DescribeInsightDetails({entity:?})"),
                describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await,
            ),
            (
                format!("ListInsights({entity:?})"),
                list(&emulator, json!({"Entity": entity.clone()})).await,
            ),
        ] {
            refused(&answer, &case, 400, "ValidationException");
            let stated = message(&answer, &case);
            assert_eq!(
                stated, expected,
                "case {case}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_entity_this_account_holds_nothing_for_is_still_refused() {
    let emulator = Emulator::new();
    create_baseline(&emulator, READY_DOMAIN).await;
    let unheld = unheld_ids();
    let insight_id = unheld
        .first()
        .expect("the id table is not empty")
        .to_owned();

    // A `DomainName` this account holds no domain under names no entity to report on, so
    // the read is refused as one missing resource — the answerable path above is the ID's,
    // not the entity's. Rows differ along the shape of the name: one never created, and one
    // at each end of the modelled length.
    for name in ["never-created", "abc", "a-domain-name-at-its-upper-b"] {
        let answer =
            describe_insight(&emulator, domain_entity(name), &insight_id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails(DomainName {name:?})"),
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        );
    }

    // An `Account` entity's `Value` is "the Amazon Web Services account ID", and the only
    // account this emulator serves is the one the request was made for.
    for foreign in ["123456789012", "210987654321"] {
        assert_ne!(
            foreign,
            engine::emulator_account_id(),
            "case DescribeInsightDetails(Account {foreign:?}): the row must name another account"
        );
        let entity = json!({"Type": "Account", "Value": foreign});
        let answer = describe_insight(&emulator, entity, &insight_id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails(Account {foreign:?})"),
            400,
            "ValidationException",
        );
    }

    // A refused read raised nothing either.
    assert!(
        holds_no_insight(&emulator),
        "case the refused reads: expected a refused read to store no insight"
    );
}
