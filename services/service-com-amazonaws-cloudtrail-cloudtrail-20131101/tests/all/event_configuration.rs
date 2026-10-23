// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutEventConfiguration` and `GetEventConfiguration`.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * `PutEventConfiguration` writes the event configuration of ONE resource and
//!   persists it, so `GetEventConfiguration` — and every later read — reports
//!   exactly what was written: the maximum event size and the context key
//!   selectors for an event data store, the aggregation configuration for a trail
//!   (model: `PutEventConfigurationRequest` / `GetEventConfigurationResponse`);
//! * `TrailName` and `EventDataStore` name resources of different kinds, so a call
//!   addresses exactly one of them and carries only the members that belong to
//!   that kind; naming both, and naming neither, are both refused with
//!   `InvalidParameterCombinationException`;
//! * a call that addresses an event data store states both the maximum event size
//!   and the context key selectors, and "if you are using context key selectors,
//!   MaxEventSize must be set to Large" (model doc for
//!   `PutEventConfigurationRequest$MaxEventSize`);
//! * an identifier that names no resource, or cannot be read at all, is refused
//!   with that resource kind's modelled error, and a refused call writes nothing;
//! * reading changes nothing a later caller sees.
//!
//! The event data store registry is seeded through the service's public `state`
//! module because `CreateEventDataStore` is still a scaffold stub (it answers
//! 501); trails are created over the wire through `CreateTrail`. Every
//! configuration call under test goes through the router.

use serde_json::{json, Value};
use std::collections::BTreeMap;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    EVENT_CATEGORY_AGGREGATION_VALUES, MAX_EVENT_SIZE_VALUES, TEMPLATE_VALUES, TYPE_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, trail_arn, EventDataStoreRecord, EventDataStoreState,
    MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE, MISSING_TRAIL_CONFIGURATION_MESSAGE,
    NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region a call
/// reaches a resource from.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` and `TrailNotFoundException`
/// `@httpError(404)`, and the recorded CloudTrail responses for this awsJson1_1
/// API carry 400.
const NOT_FOUND_STATUS: u16 = 400;

/// The model bounds `ContextKeySelectors` to 0..=2 entries, so two selectors is
/// the top of that range.
const MAX_CONTEXT_KEY_SELECTORS: usize = 2;

/// The model bounds `ContextKeySelector$Equals` to 1..=50 keys.
const MAX_EQUALS_KEYS: usize = 50;

/// The id of the store every store case configures.
const STORE_ID: &str = "11111111-1111-4111-8111-111111111111";

/// The id of a store in [`OTHER_REGION`], for the Region cases.
const ELSEWHERE_STORE_ID: &str = "22222222-2222-4222-8222-222222222222";

/// The id of a store awaiting deletion, for the inactive-store case.
const PENDING_DELETION_STORE_ID: &str = "33333333-3333-4333-8333-333333333333";

/// A store record under `id` in `region`, as `CreateEventDataStore` will persist
/// one once it is implemented.
fn store_record(id: &str, region: &str, status: EventDataStoreState) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status,
        ..EventDataStoreRecord::default()
    }
}

/// Seed the three stores every case addresses.
fn seed_stores(emulator: &Emulator) {
    let stores = [
        store_record(STORE_ID, HOME_REGION, EventDataStoreState::Enabled),
        store_record(ELSEWHERE_STORE_ID, OTHER_REGION, EventDataStoreState::Enabled),
        store_record(
            PENDING_DELETION_STORE_ID,
            HOME_REGION,
            EventDataStoreState::PendingDeletion,
        ),
    ];
    let state = emulator.state();
    let mut store = state.lock();
    for record in stores {
        store.event_data_stores.insert(record.id.clone(), record);
    }
}

/// Create `name` in `region` over the wire, as a client does.
async fn create_trail(emulator: &Emulator, region: &str, name: &str) {
    let reply = emulator
        .call_in(
            "CreateTrail",
            region,
            &json!({ "Name": name, "S3BucketName": "configuration-bucket" }),
        )
        .await;
    reply.expect_ok(&format!("CreateTrail {name} in {region}"));
}

/// One context key selector, as the wire carries it.
fn selector(r#type: &str, equals: &[String]) -> Value {
    json!({ "Type": r#type, "Equals": equals })
}

/// One aggregation configuration, as the wire carries it.
fn aggregation(templates: &[&str], event_category: &str) -> Value {
    json!({ "Templates": templates, "EventCategory": event_category })
}

/// `count` distinct context keys, for the `Equals` bound case.
fn keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("ctx:Key{index}")).collect()
}

/// The event data store configurations a round trip must report back, differing
/// along the members the call carries: each `MaxEventSize` value, no selectors,
/// one selector of each `Type` the model admits, the top of the
/// `ContextKeySelectors` range, and a selector at the top of the `Equals` range.
fn store_configurations() -> Vec<(String, &'static str, Vec<Value>)> {
    let request_context = TYPE_VALUES[0];
    let tag_context = TYPE_VALUES[1];
    let standard = "Standard";
    let large = "Large";
    // Both values of the model's `MaxEventSize` enum are covered by the rows
    // below; this asserts the catalog the rows were written against.
    assert_eq!(
        MAX_EVENT_SIZE_VALUES.len(),
        2,
        "MaxEventSize is modelled with exactly two values, got {MAX_EVENT_SIZE_VALUES:?}"
    );

    vec![
        (
            format!("{standard}, no selectors"),
            standard,
            Vec::new(),
        ),
        (format!("{large}, no selectors"), large, Vec::new()),
        (
            format!("{large}, one {request_context} selector"),
            large,
            vec![selector(request_context, &keys(1))],
        ),
        (
            format!("{large}, one {tag_context} selector"),
            large,
            vec![selector(tag_context, &keys(3))],
        ),
        (
            format!("{large}, {MAX_CONTEXT_KEY_SELECTORS} selectors"),
            large,
            vec![
                selector(request_context, &keys(2)),
                selector(tag_context, &keys(1)),
            ],
        ),
        (
            format!("{large}, a selector with {MAX_EQUALS_KEYS} keys"),
            large,
            vec![selector(request_context, &keys(MAX_EQUALS_KEYS))],
        ),
    ]
}

/// The trail configurations a round trip must report back: one row per
/// `Template` value the model admits, plus a configuration carrying every
/// template at once. `EventCategoryAggregation` models the single value `Data`.
fn trail_configurations() -> Vec<(String, Vec<Value>)> {
    let category = EVENT_CATEGORY_AGGREGATION_VALUES[0];
    let mut rows: Vec<(String, Vec<Value>)> = TEMPLATE_VALUES
        .iter()
        .map(|template| {
            (
                format!("template {template}, category {category}"),
                vec![aggregation(&[template], category)],
            )
        })
        .collect();
    rows.push((
        format!("every template, category {category}"),
        vec![aggregation(TEMPLATE_VALUES, category)],
    ));
    rows
}

/// Call `PutEventConfiguration` from `region`.
async fn put_in(emulator: &Emulator, region: &str, body: &Value) -> Reply {
    emulator
        .call_in("PutEventConfiguration", region, body)
        .await
}

/// Call `GetEventConfiguration` from `region`.
async fn get_in(emulator: &Emulator, region: &str, body: &Value) -> Reply {
    emulator
        .call_in("GetEventConfiguration", region, body)
        .await
}

/// The error `Message` a reply carries, or `""`.
fn message(reply: &Reply) -> &str {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Every stored trail and event data store, rendered — for the cases that assert
/// a call left the registries alone.
fn registries(emulator: &Emulator) -> BTreeMap<String, String> {
    let state = emulator.state();
    let store = state.lock();
    store
        .event_data_stores
        .iter()
        .map(|(id, record)| (format!("store/{id}"), format!("{record:?}")))
        .chain(
            store
                .trails
                .iter()
                .map(|(key, record)| (format!("trail/{key:?}"), format!("{record:?}"))),
        )
        .collect()
}

/// The configuration body a store call answers with: the ARN, the size, and the
/// selectors when it carries any.
fn expected_store_answer(id: &str, region: &str, max_event_size: &str, selectors: &[Value]) -> Value {
    let mut expected = json!({
        "EventDataStoreArn": event_data_store_arn(region, id),
        "MaxEventSize": max_event_size,
    });
    if !selectors.is_empty() {
        expected["ContextKeySelectors"] = json!(selectors);
    }
    expected
}

#[tokio::test]
async fn an_event_data_store_configuration_is_persisted_and_read_back() {
    let emulator = Emulator::new();
    seed_stores(&emulator);
    let arn = event_data_store_arn(HOME_REGION, STORE_ID);

    for (case, max_event_size, selectors) in store_configurations() {
        // The identifier is an ARN or the id suffix of one, so the call can be
        // written either way; the write goes in under one form and the read back
        // under the other.
        let written = put_in(
            &emulator,
            HOME_REGION,
            &json!({
                "EventDataStore": STORE_ID,
                "MaxEventSize": max_event_size,
                "ContextKeySelectors": selectors,
            }),
        )
        .await;
        let expected = expected_store_answer(STORE_ID, HOME_REGION, max_event_size, &selectors);
        assert_eq!(
            written.expect_ok(&case),
            &expected,
            "case {case:?}: the write must report the stored configuration {expected:?}, \
             got {:?}",
            written.body
        );

        let read = get_in(&emulator, HOME_REGION, &json!({ "EventDataStore": arn })).await;
        assert_eq!(
            read.expect_ok(&case),
            &expected,
            "case {case:?}: the read must report what was written ({expected:?}), got {:?}",
            read.body
        );

        // The stored record is what the answer came from.
        let state = emulator.state();
        let stored = state.lock().event_data_stores[STORE_ID].clone();
        assert_eq!(
            stored.max_event_size.as_wire(),
            max_event_size,
            "case {case:?}: the store must persist MaxEventSize {max_event_size:?}, got {:?}",
            stored.max_event_size
        );
        assert_eq!(
            stored.context_key_selectors.len(),
            selectors.len(),
            "case {case:?}: the store must persist {} selector(s), got {:?}",
            selectors.len(),
            stored.context_key_selectors
        );
    }
}

#[tokio::test]
async fn a_trail_configuration_is_persisted_and_read_back() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "aggregating-trail").await;
    let arn = trail_arn(HOME_REGION, "aggregating-trail");

    for (case, configurations) in trail_configurations() {
        let expected = json!({
            "TrailARN": arn,
            "AggregationConfigurations": configurations,
        });

        // A trail is addressed by bare name or by trail ARN: the write uses the
        // name and the read back the ARN, and both answer the same configuration.
        let written = put_in(
            &emulator,
            HOME_REGION,
            &json!({
                "TrailName": "aggregating-trail",
                "AggregationConfigurations": configurations,
            }),
        )
        .await;
        assert_eq!(
            written.expect_ok(&case),
            &expected,
            "case {case:?}: the write must report the stored configuration {expected:?}, \
             got {:?}",
            written.body
        );

        let read = get_in(&emulator, HOME_REGION, &json!({ "TrailName": arn })).await;
        assert_eq!(
            read.expect_ok(&case),
            &expected,
            "case {case:?}: the read must report what was written ({expected:?}), got {:?}",
            read.body
        );
    }

    // A trail nobody configured aggregation for carries none, so the member stays
    // absent rather than becoming an empty list.
    create_trail(&emulator, HOME_REGION, "unconfigured-trail").await;
    let read = get_in(
        &emulator,
        HOME_REGION,
        &json!({ "TrailName": "unconfigured-trail" }),
    )
    .await;
    assert_eq!(
        read.expect_ok("unconfigured trail"),
        &json!({ "TrailARN": trail_arn(HOME_REGION, "unconfigured-trail") }),
        "case \"unconfigured trail\": a trail with no aggregation configuration must report \
         only its ARN, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn a_later_write_replaces_the_configuration_a_read_reports() {
    let emulator = Emulator::new();
    seed_stores(&emulator);

    // Every row applied to the same store in turn: each write is what the next
    // read reports, so the configuration is replaced and never merged.
    for (case, max_event_size, selectors) in store_configurations() {
        put_in(
            &emulator,
            HOME_REGION,
            &json!({
                "EventDataStore": STORE_ID,
                "MaxEventSize": max_event_size,
                "ContextKeySelectors": selectors,
            }),
        )
        .await
        .expect_ok(&case);

        let read = get_in(
            &emulator,
            HOME_REGION,
            &json!({ "EventDataStore": STORE_ID }),
        )
        .await;
        let expected = expected_store_answer(STORE_ID, HOME_REGION, max_event_size, &selectors);
        assert_eq!(
            read.expect_ok(&case),
            &expected,
            "case {case:?}: a read must report the last configuration written \
             ({expected:?}), got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn reading_a_configuration_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_stores(&emulator);
    create_trail(&emulator, HOME_REGION, "aggregating-trail").await;
    put_in(
        &emulator,
        HOME_REGION,
        &json!({
            "EventDataStore": STORE_ID,
            "MaxEventSize": "Large",
            "ContextKeySelectors": [selector(TYPE_VALUES[0], &keys(1))],
        }),
    )
    .await
    .expect_ok("seed the store configuration");
    put_in(
        &emulator,
        HOME_REGION,
        &json!({
            "TrailName": "aggregating-trail",
            "AggregationConfigurations": [aggregation(
                &[TEMPLATE_VALUES[0]],
                EVENT_CATEGORY_AGGREGATION_VALUES[0],
            )],
        }),
    )
    .await
    .expect_ok("seed the trail configuration");
    let before = registries(&emulator);

    for (case, request) in [
        ("store", json!({ "EventDataStore": STORE_ID })),
        ("trail", json!({ "TrailName": "aggregating-trail" })),
    ] {
        let first = get_in(&emulator, HOME_REGION, &request).await;
        let first = first.expect_ok(case).clone();
        for repeat in 1..=3 {
            let case = format!("{case}, read {repeat}");
            let again = get_in(&emulator, HOME_REGION, &request).await;
            assert_eq!(
                again.expect_ok(&case),
                &first,
                "case {case:?}: a read must answer what the first read answered, got {:?}",
                again.body
            );
        }
    }

    assert_eq!(
        registries(&emulator),
        before,
        "the registries must be untouched by reads, got {:?}",
        registries(&emulator)
    );
}

#[tokio::test]
async fn a_configuration_call_that_breaks_a_rule_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    seed_stores(&emulator);
    create_trail(&emulator, HOME_REGION, "aggregating-trail").await;
    create_trail(&emulator, OTHER_REGION, "trail-elsewhere").await;
    let before = registries(&emulator);
    let selectors = json!([selector(TYPE_VALUES[0], &keys(1))]);
    let aggregations = json!([aggregation(
        &[TEMPLATE_VALUES[0]],
        EVENT_CATEGORY_AGGREGATION_VALUES[0]
    )]);

    // Each row: the case, the request, and the HTTP status and error code the
    // rule it breaks is refused with.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no resource at all",
            json!({ "MaxEventSize": "Standard", "ContextKeySelectors": [] }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "both resources",
            json!({
                "TrailName": "aggregating-trail",
                "EventDataStore": STORE_ID,
                "AggregationConfigurations": aggregations,
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a store with neither the size nor the selectors",
            json!({ "EventDataStore": STORE_ID }),
            400,
            "ValidationException",
        ),
        (
            "a store with the size but no selectors member",
            json!({ "EventDataStore": STORE_ID, "MaxEventSize": "Large" }),
            400,
            "ValidationException",
        ),
        (
            "a store with selectors but no size",
            json!({ "EventDataStore": STORE_ID, "ContextKeySelectors": selectors }),
            400,
            "ValidationException",
        ),
        (
            "a store with a trail-level member",
            json!({
                "EventDataStore": STORE_ID,
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
                "AggregationConfigurations": aggregations,
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "selectors with a Standard maximum event size",
            json!({
                "EventDataStore": STORE_ID,
                "MaxEventSize": "Standard",
                "ContextKeySelectors": selectors,
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a trail with no aggregation configuration",
            json!({ "TrailName": "aggregating-trail" }),
            400,
            "ValidationException",
        ),
        (
            "a trail with a store-level member",
            json!({
                "TrailName": "aggregating-trail",
                "AggregationConfigurations": aggregations,
                "MaxEventSize": "Large",
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a trail with store-level selectors",
            json!({
                "TrailName": "aggregating-trail",
                "AggregationConfigurations": aggregations,
                "ContextKeySelectors": selectors,
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a store id no store carries",
            json!({
                "EventDataStore": "44444444-4444-4444-8444-444444444444",
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
            }),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "a store living in another Region, by bare id",
            json!({
                "EventDataStore": ELSEWHERE_STORE_ID,
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
            }),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "an identifier that is neither a store ARN nor a store id",
            json!({
                "EventDataStore": format!(
                    "arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{STORE_ID}"
                ),
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
            }),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "a store awaiting deletion",
            json!({
                "EventDataStore": PENDING_DELETION_STORE_ID,
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
            }),
            400,
            "InactiveEventDataStoreException",
        ),
        (
            "a trail name no trail carries",
            json!({
                "TrailName": "absent-trail",
                "AggregationConfigurations": aggregations,
            }),
            NOT_FOUND_STATUS,
            "TrailNotFoundException",
        ),
        (
            "a trail homed in another Region",
            json!({
                "TrailName": "trail-elsewhere",
                "AggregationConfigurations": aggregations,
            }),
            400,
            "InvalidHomeRegionException",
        ),
        (
            "a trail name in IP-address format",
            json!({
                "TrailName": "192.168.5.4",
                "AggregationConfigurations": aggregations,
            }),
            400,
            "InvalidTrailNameException",
        ),
        (
            "a trail name ending in a dash",
            json!({
                "TrailName": "trailing-dash-",
                "AggregationConfigurations": aggregations,
            }),
            400,
            "InvalidTrailNameException",
        ),
        (
            "an ARN whose resource part is not trail/<name>",
            json!({
                "TrailName": format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:badtrail"),
                "AggregationConfigurations": aggregations,
            }),
            400,
            "CloudTrailARNInvalidException",
        ),
    ];

    for (case, request, status, code) in cases {
        let reply = put_in(&emulator, HOME_REGION, &request).await;
        reply.expect_error(case, status, code);
        assert!(
            !message(&reply).is_empty(),
            "case {case:?}: a refusal must say what was wrong with the request, got {:?}",
            reply.body
        );
    }

    assert_eq!(
        registries(&emulator),
        before,
        "a refused configuration call must write nothing, got {:?}",
        registries(&emulator)
    );
}

#[tokio::test]
async fn the_documented_refusals_say_which_rule_the_request_broke() {
    let emulator = Emulator::new();
    seed_stores(&emulator);
    create_trail(&emulator, HOME_REGION, "aggregating-trail").await;

    // The wording of these two refusals is the service's own: the first is how it
    // words the "exactly one resource" rule, the second how it words the
    // configuration an event data store call must state.
    let cases = [
        (
            "no resource at all",
            json!({}),
            NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE,
        ),
        (
            "both resources",
            json!({ "TrailName": "aggregating-trail", "EventDataStore": STORE_ID }),
            NO_SINGLE_EVENT_CONFIGURATION_RESOURCE_MESSAGE,
        ),
        (
            "a store with no configuration",
            json!({ "EventDataStore": STORE_ID }),
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE,
        ),
        (
            "a trail with no configuration",
            json!({ "TrailName": "aggregating-trail" }),
            MISSING_TRAIL_CONFIGURATION_MESSAGE,
        ),
    ];

    for (case, request, expected) in cases {
        let reply = put_in(&emulator, HOME_REGION, &request).await;
        assert_eq!(
            message(&reply),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_read_of_a_resource_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    seed_stores(&emulator);
    create_trail(&emulator, HOME_REGION, "aggregating-trail").await;

    // The read applies the same identity and identifier rules as the write.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no resource at all",
            json!({}),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "both resources",
            json!({ "TrailName": "aggregating-trail", "EventDataStore": STORE_ID }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a store id no store carries",
            json!({ "EventDataStore": "44444444-4444-4444-8444-444444444444" }),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "an unreadable store identifier",
            json!({ "EventDataStore": "eventdatastore/" }),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "a trail name no trail carries",
            json!({ "TrailName": "absent-trail" }),
            NOT_FOUND_STATUS,
            "TrailNotFoundException",
        ),
        (
            "a trail name in IP-address format",
            json!({ "TrailName": "192.168.5.4" }),
            400,
            "InvalidTrailNameException",
        ),
        (
            "an ARN whose resource part is not trail/<name>",
            json!({ "TrailName": format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:badtrail") }),
            400,
            "CloudTrailARNInvalidException",
        ),
    ];

    for (case, request, status, code) in cases {
        let reply = get_in(&emulator, HOME_REGION, &request).await;
        reply.expect_error(case, status, code);
    }
}

#[tokio::test]
async fn a_store_and_a_trail_in_another_region_are_reached_by_calling_there() {
    let emulator = Emulator::new();
    seed_stores(&emulator);
    create_trail(&emulator, OTHER_REGION, "trail-elsewhere").await;
    let selectors = json!([selector(TYPE_VALUES[1], &keys(2))]);
    let aggregations = json!([aggregation(
        &[TEMPLATE_VALUES[1]],
        EVENT_CATEGORY_AGGREGATION_VALUES[0]
    )]);

    // The resource lives in another Region, so the call that configures it is
    // made there — and the answer names that Region's ARNs.
    let written = put_in(
        &emulator,
        OTHER_REGION,
        &json!({
            "EventDataStore": ELSEWHERE_STORE_ID,
            "MaxEventSize": "Large",
            "ContextKeySelectors": selectors,
        }),
    )
    .await;
    assert_eq!(
        written.expect_ok("store in another Region"),
        &json!({
            "EventDataStoreArn": event_data_store_arn(OTHER_REGION, ELSEWHERE_STORE_ID),
            "MaxEventSize": "Large",
            "ContextKeySelectors": selectors,
        }),
        "case \"store in another Region\": the answer must name that Region's store, got {:?}",
        written.body
    );

    let written = put_in(
        &emulator,
        OTHER_REGION,
        &json!({
            "TrailName": "trail-elsewhere",
            "AggregationConfigurations": aggregations,
        }),
    )
    .await;
    assert_eq!(
        written.expect_ok("trail in another Region"),
        &json!({
            "TrailARN": trail_arn(OTHER_REGION, "trail-elsewhere"),
            "AggregationConfigurations": aggregations,
        }),
        "case \"trail in another Region\": the answer must name that Region's trail, got {:?}",
        written.body
    );

    // A read from the other Region reports the same configuration, and the
    // store's ARN addresses it from anywhere.
    let read = get_in(
        &emulator,
        HOME_REGION,
        &json!({ "EventDataStore": event_data_store_arn(OTHER_REGION, ELSEWHERE_STORE_ID) }),
    )
    .await;
    assert_eq!(
        read.expect_ok("store read by ARN from another Region"),
        &json!({
            "EventDataStoreArn": event_data_store_arn(OTHER_REGION, ELSEWHERE_STORE_ID),
            "MaxEventSize": "Large",
            "ContextKeySelectors": selectors,
        }),
        "case \"store read by ARN from another Region\": the ARN pins the store's Region, \
         got {:?}",
        read.body
    );
}
