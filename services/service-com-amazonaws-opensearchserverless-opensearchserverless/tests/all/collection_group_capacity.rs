// SPDX-License-Identifier: CC0-1.0

//! A collection group's EFFECTIVE settings: the `capacityLimits` every detail reports and
//! the `storageConfiguration` beside it, end to end over the wire.
//!
//! The rule under test: a collection group always HAS effective capacity limits, so every
//! detail shape that models `capacityLimits` reports them — each member the create request
//! named, kept as given, plus the service default for each CEILING the request did not
//! name — and reports a `storageConfiguration` whose `v2Enabled` is computed from the
//! group's own stored `generation`. The create answer and every later read of the same
//! group agree, because both are rendered by the stored record.
//!
//! The tables below span what the rule generalises over: the shape of the requested
//! `capacityLimits` (absent, empty, one member, each pair, all four), every
//! `ServerlessGeneration` member plus the omitted case, both `StandbyReplicas` members,
//! the write that states limits (`CreateCollectionGroup`) and the write that restates them
//! (`UpdateCollectionGroup`), and id- as well as name-addressed reads. The defaults
//! themselves are read from the handler's own constants rather than copied into every row,
//! and ONE test (`the_defaulted_ceilings_are_the_ones_the_service_fills_in`) pins the
//! recorded pair as a literal, so the constants cannot drift from the observed service
//! unnoticed.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
    DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU,
    DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU, SERVERLESS_GENERATION_CLASSIC,
    SERVERLESS_GENERATION_NEXTGEN,
};

use crate::collection_lifecycle::{at, call, call_in, emulator, text};

/// The four `CollectionGroupCapacityLimits` members, from the shape's own member list
/// (`smithy_query(query="shape", shape="CollectionGroupCapacityLimits")`).
const CEILINGS: [&str; 2] = ["maxIndexingCapacityInOCU", "maxSearchCapacityInOCU"];
const FLOORS: [&str; 2] = ["minIndexingCapacityInOCU", "minSearchCapacityInOCU"];

/// The default this service fills a missing ceiling with, as a JSON number.
fn default_ceiling(member: &str) -> Value {
    let ocu = match member {
        "maxIndexingCapacityInOCU" => DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU,
        "maxSearchCapacityInOCU" => DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU,
        other => panic!("the table named an unmodelled ceiling member {other}"),
    };
    json!(f64::from(ocu))
}

/// The effective `capacityLimits` a group created with `requested` must report: every
/// member `requested` names kept as given, each unnamed ceiling defaulted, and a floor
/// `requested` does not name left absent.
pub(crate) fn expected_capacity_limits(requested: &Value) -> Value {
    let mut expected = serde_json::Map::new();
    for ceiling in CEILINGS {
        let value = match at(requested, ceiling) {
            Value::Null => default_ceiling(ceiling),
            named => named.clone(),
        };
        expected.insert(ceiling.to_string(), value);
    }
    for floor in FLOORS {
        let named = at(requested, floor);
        if named.is_number() {
            expected.insert(floor.to_string(), named.clone());
        }
    }
    Value::Object(expected)
}

/// The `storageConfiguration` a group of `generation` must report.
fn expected_storage_configuration(generation: &str) -> Value {
    json!({ "v2Enabled": generation == SERVERLESS_GENERATION_NEXTGEN })
}

#[tokio::test]
async fn every_collection_group_reports_effective_capacity_limits_and_storage_configuration() {
    let router = emulator();

    // (case, the `capacityLimits` the request names, the `generation` it names,
    //  `standbyReplicas`). A `None` generation is the omitted case, which stores CLASSIC.
    let cases: Vec<(&str, Value, Option<&str>, &str)> = vec![
        ("no capacityLimits member at all", Value::Null, None, "ENABLED"),
        ("an empty capacityLimits object", json!({}), Some("CLASSIC"), "DISABLED"),
        (
            "only the indexing ceiling",
            json!({ "maxIndexingCapacityInOCU": 4.0 }),
            Some("NEXTGEN"),
            "ENABLED",
        ),
        (
            "only the search ceiling",
            json!({ "maxSearchCapacityInOCU": 12.0 }),
            None,
            "DISABLED",
        ),
        (
            "only the indexing floor",
            json!({ "minIndexingCapacityInOCU": 2.0 }),
            Some("NEXTGEN"),
            "DISABLED",
        ),
        (
            "only the search floor",
            json!({ "minSearchCapacityInOCU": 0.0 }),
            Some("CLASSIC"),
            "ENABLED",
        ),
        (
            "both floors and no ceiling",
            json!({ "minIndexingCapacityInOCU": 1.5, "minSearchCapacityInOCU": 2.5 }),
            None,
            "ENABLED",
        ),
        (
            "both ceilings and no floor",
            json!({ "maxIndexingCapacityInOCU": 8.0, "maxSearchCapacityInOCU": 16.0 }),
            Some("NEXTGEN"),
            "DISABLED",
        ),
        (
            "all four members",
            json!({
                "minIndexingCapacityInOCU": 2.0,
                "maxIndexingCapacityInOCU": 6.0,
                "minSearchCapacityInOCU": 4.0,
                "maxSearchCapacityInOCU": 4.0,
            }),
            Some("CLASSIC"),
            "ENABLED",
        ),
        (
            // A floor AT the defaulted ceiling: the published value itself.
            "a floor at the defaulted ceiling",
            json!({
                "minSearchCapacityInOCU": DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU,
            }),
            None,
            "ENABLED",
        ),
    ];

    for (index, (case, limits, generation, standby)) in cases.into_iter().enumerate() {
        let name = format!("capacity-group-{index}");
        let mut request = json!({ "name": &name, "standbyReplicas": standby });
        if let Value::Object(_) = &limits {
            request["capacityLimits"] = limits.clone();
        }
        if let Some(generation) = generation {
            request["generation"] = json!(generation);
        }

        let (status, code, answer) = call(&router, "CreateCollectionGroup", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "createCollectionGroupDetail").clone();

        let expected_limits = expected_capacity_limits(&limits);
        assert_eq!(
            at(&detail, "capacityLimits"),
            &expected_limits,
            "case {case:?}: the create must report the effective capacityLimits, got {detail}"
        );
        for ceiling in CEILINGS {
            assert!(
                at(&detail, &format!("capacityLimits.{ceiling}"))
                    .as_f64()
                    .is_some(),
                "case {case:?}: {ceiling} must be present on every group, got {detail}"
            );
        }
        for floor in FLOORS {
            assert_eq!(
                at(&detail, &format!("capacityLimits.{floor}")).is_null(),
                at(&limits, floor).is_null(),
                "case {case:?}: {floor} must be present exactly when the request named it, \
                 got {detail}"
            );
        }

        let stored_generation = generation.unwrap_or(SERVERLESS_GENERATION_CLASSIC);
        assert_eq!(
            text(&detail, "generation"),
            stored_generation,
            "case {case:?}: generation must be the stored one, got {detail}"
        );
        assert_eq!(
            at(&detail, "storageConfiguration"),
            &expected_storage_configuration(stored_generation),
            "case {case:?}: storageConfiguration must follow the stored generation, got {detail}"
        );

        // The read path reports exactly what the create did, addressed either way.
        let id = text(&detail, "id");
        for selector in [json!({ "ids": [&id] }), json!({ "names": [&name] })] {
            let case = format!("{case:?} read back with {selector}");
            let (status, code, answer) =
                call_in(&router, "us-east-1", "BatchGetCollectionGroup", selector).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {code} with {answer}"
            );
            let read = at(&answer, "collectionGroupDetails")
                .as_array()
                .and_then(|details| details.first())
                .cloned()
                .unwrap_or(Value::Null);
            for member in ["capacityLimits", "storageConfiguration"] {
                assert_eq!(
                    at(&read, member),
                    at(&detail, member),
                    "case {case}: {member} must be what the create reported, got {read}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_defaulted_ceilings_are_the_ones_the_service_fills_in() {
    let router = emulator();

    // The recorded default, pinned as a literal here and nowhere else in this suite: the
    // live API answers a create that named no `capacityLimits` with
    // {"maxIndexingCapacityInOCU": 96.0, "maxSearchCapacityInOCU": 96.0} and no floor.
    // Every other expectation is derived from the handler's constants, so this one row is
    // what ties those constants to the observed service.
    const RECORDED_DEFAULT: f64 = 96.0;

    let (status, code, answer) = call(
        &router,
        "CreateCollectionGroup",
        json!({ "name": "recorded-default-group", "standbyReplicas": "DISABLED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the recorded default: expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        at(at(&answer, "createCollectionGroupDetail"), "capacityLimits"),
        &json!({
            "maxIndexingCapacityInOCU": RECORDED_DEFAULT,
            "maxSearchCapacityInOCU": RECORDED_DEFAULT,
        }),
        "case the recorded default: a create naming no capacityLimits must report the \
         service default ceilings and no floor, got {answer}"
    );
    assert_eq!(
        at(
            at(&answer, "createCollectionGroupDetail"),
            "storageConfiguration"
        ),
        &json!({ "v2Enabled": false }),
        "case the recorded default: a CLASSIC group must report v2Enabled false, got {answer}"
    );
    // And the constants the rest of this suite derives its expectations from are those
    // same numbers, so neither can drift from the other unnoticed.
    assert_eq!(
        (
            f64::from(DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU),
            f64::from(DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU)
        ),
        (RECORDED_DEFAULT, RECORDED_DEFAULT),
        "case the recorded default: the service's own default constants must be the \
         recorded pair"
    );
}

#[tokio::test]
async fn a_floor_above_the_defaulted_ceiling_is_stored_rather_than_refused() {
    let router = emulator();

    // A ceiling the service defaults in is not a bound the caller was held to, so a floor
    // named above it is accepted and reported as given. One past each published default,
    // and far past one.
    let over_indexing = f64::from(DEFAULT_COLLECTION_GROUP_MAX_INDEXING_CAPACITY_IN_OCU) + 1.0;
    let over_search = f64::from(DEFAULT_COLLECTION_GROUP_MAX_SEARCH_CAPACITY_IN_OCU) + 1.0;
    let cases: Vec<(&str, Value)> = vec![
        (
            "an indexing floor one past the defaulted ceiling",
            json!({ "minIndexingCapacityInOCU": over_indexing }),
        ),
        (
            "a search floor one past the defaulted ceiling",
            json!({ "minSearchCapacityInOCU": over_search }),
        ),
        (
            "an indexing floor far above the defaulted ceiling",
            json!({ "minIndexingCapacityInOCU": over_indexing * 4.0 }),
        ),
    ];

    for (index, (case, limits)) in cases.into_iter().enumerate() {
        let name = format!("high-floor-group-{index}");
        let (status, code, answer) = call(
            &router,
            "CreateCollectionGroup",
            json!({
                "name": &name,
                "standbyReplicas": "ENABLED",
                "capacityLimits": limits.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(at(&answer, "createCollectionGroupDetail"), "capacityLimits"),
            &expected_capacity_limits(&limits),
            "case {case:?}: the floor must be stored as named beside the defaulted \
             ceilings, got {answer}"
        );
    }

    // What IS refused is a contradiction inside ONE request: a floor above the ceiling the
    // same request named, for either half.
    for (case, limits) in [
        (
            "an indexing floor above the ceiling the request named",
            json!({ "minIndexingCapacityInOCU": 8.0, "maxIndexingCapacityInOCU": 4.0 }),
        ),
        (
            "a search floor above the ceiling the request named",
            json!({ "minSearchCapacityInOCU": 8.0, "maxSearchCapacityInOCU": 4.0 }),
        ),
    ] {
        let (status, code, answer) = call(
            &router,
            "CreateCollectionGroup",
            json!({
                "name": "contradictory-group",
                "standbyReplicas": "ENABLED",
                "capacityLimits": limits,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // The refusals stored nothing: the name they used is still free.
    let (status, code, answer) = call(
        &router,
        "CreateCollectionGroup",
        json!({ "name": "contradictory-group", "standbyReplicas": "ENABLED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a refused name stays free: expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        at(at(&answer, "createCollectionGroupDetail"), "capacityLimits"),
        &expected_capacity_limits(&Value::Null),
        "case a refused name stays free: expected the defaulted ceilings, got {answer}"
    );
}

#[tokio::test]
async fn an_update_that_restates_the_limits_leaves_the_group_with_effective_limits() {
    let router = emulator();

    // The limits every group starts with: all four members named, so nothing is defaulted
    // until an update restates them.
    let created = json!({
        "minIndexingCapacityInOCU": 2.0,
        "maxIndexingCapacityInOCU": 8.0,
        "minSearchCapacityInOCU": 2.0,
        "maxSearchCapacityInOCU": 8.0,
    });

    // (case, the `capacityLimits` the update states). `UpdateCollectionGroupRequest`
    // carries the limits as ONE structure, so what it states is what the group has — under
    // the same rule the create follows.
    let cases: Vec<(&str, Value)> = vec![
        ("an update stating only a floor", json!({ "minIndexingCapacityInOCU": 1.0 })),
        ("an update stating only a ceiling", json!({ "maxSearchCapacityInOCU": 32.0 })),
        ("an update stating an empty structure", json!({})),
        (
            "an update stating every member",
            json!({
                "minIndexingCapacityInOCU": 0.0,
                "maxIndexingCapacityInOCU": 3.0,
                "minSearchCapacityInOCU": 1.0,
                "maxSearchCapacityInOCU": 3.0,
            }),
        ),
    ];

    for (index, (case, limits)) in cases.into_iter().enumerate() {
        let name = format!("updated-capacity-group-{index}");
        let (status, code, answer) = call(
            &router,
            "CreateCollectionGroup",
            json!({
                "name": &name,
                "standbyReplicas": "ENABLED",
                "capacityLimits": created.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must be accepted, got {status} {code} with {answer}"
        );
        let id = text(at(&answer, "createCollectionGroupDetail"), "id");

        let (status, code, answer) = call(
            &router,
            "UpdateCollectionGroup",
            json!({ "id": &id, "capacityLimits": limits.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {code} with {answer}"
        );
        let expected_limits = expected_capacity_limits(&limits);
        assert_eq!(
            at(at(&answer, "updateCollectionGroupDetail"), "capacityLimits"),
            &expected_limits,
            "case {case:?}: the update must report the effective limits, got {answer}"
        );

        let (status, code, answer) =
            call(&router, "BatchGetCollectionGroup", json!({ "ids": [&id] })).await;
        assert_eq!(
            status, 200,
            "case {case:?} read back: expected 200, got {status} {code} with {answer}"
        );
        let read = at(&answer, "collectionGroupDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            at(&read, "capacityLimits"),
            &expected_limits,
            "case {case:?} read back: the read must report the written limits, got {read}"
        );
    }
}
