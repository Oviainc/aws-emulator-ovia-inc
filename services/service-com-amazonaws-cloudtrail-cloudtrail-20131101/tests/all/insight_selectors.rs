// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutInsightSelectors` / `GetInsightSelectors`.
//!
//! Every case goes over the wire through the shared harness in [`super::emulator`] —
//! an `awsJson1_1` `POST /` with this service's own `X-Amz-Target` prefix, dispatched
//! by the same `MultiServiceRouter` the production router serves — so what is asserted
//! here is what a client sees, including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `PutInsightSelectors` persists the `InsightSelectors` statement on the resource it
//!   addresses and `GetInsightSelectors` reports exactly that, with the documented
//!   default of a member the request left out filled in ("If EventCategories is not
//!   provided, the specified Insights types are enabled on management API calls by
//!   default");
//! * the statement REPLACES whatever was configured before, and an EMPTY statement
//!   turns Insights event logging off — after which `HasInsightSelectors` reads false
//!   and the read answers `InsightNotEnabledException`;
//! * an `InsightType` the model's `InsightType` enum does not carry is refused with
//!   HTTP 400 `InvalidInsightSelectorsException`, naming the value sent and the value
//!   set, and changes nothing;
//! * `TrailName` and `EventDataStore` address resources of different kinds, so exactly
//!   one of them addresses a call, `InsightsDestination` belongs to the event data
//!   store form and is required there, and every other combination is
//!   `InvalidParameterCombinationException`;
//! * both operations read a trail identifier as a bare name or a trail ARN and refuse a
//!   malformed one as the form it was written in, while the write — unlike the read —
//!   runs only in the trail's home Region;
//! * Insights on an event data store need a source that logs management events and a
//!   destination that logs Insights events, and are not supported on its data events;
//! * reading changes nothing a later caller sees.
//!
//! The event data store registry is seeded through the service's public `state` module
//! because `CreateEventDataStore` is still a scaffold stub (it answers 501), so those
//! records cannot yet be made over the wire. Every call under test still goes through
//! the router.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    AdvancedEventSelector, AdvancedFieldSelector, INSIGHT_TYPE_VALUES,
    SOURCE_EVENT_CATEGORY_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, invalid_insight_type_message, EventDataStoreRecord, EventDataStoreState,
    DATA_INSIGHTS_EVENT_CATEGORY, DEFAULT_INSIGHTS_EVENT_CATEGORY, EVENT_CATEGORY_FIELD,
    INSIGHT_EVENT_CATEGORY,
};

use super::emulator::{Emulator, HOME_REGION};

/// A Region that is nobody's home Region in these cases.
const OTHER_REGION: &str = "eu-west-1";

/// One row of a statement table: the case, the Insights types the statement names, and
/// the `EventCategories` each of them selects — `None` for a request that leaves the
/// member out, which selects the documented default.
type StatementRow<Case> = (Case, Vec<&'static str>, Option<Vec<&'static str>>);

/// The id of the source event data store the event-data-store cases address.
const SOURCE_ID: &str = "11111111-1111-4111-8111-111111111111";
/// The id of the destination event data store the event-data-store cases log to.
const DESTINATION_ID: &str = "22222222-2222-4222-8222-222222222222";

/// Create a trail and hand back the ARN the service minted for it.
async fn create_trail(emulator: &Emulator, region: &str, name: &str) -> String {
    let request = json!({"Name": name, "S3BucketName": format!("bucket-for-{name}")});
    let created = emulator.call_in("CreateTrail", region, &request).await;
    created
        .expect_ok(&format!("CreateTrail {request}"))
        .get("TrailARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An event data store that collects `categories`, in `region`.
///
/// Which categories a store collects is what its advanced event selectors state, on
/// the `eventCategory` field (model doc for `PutInsightSelectors`: "You can call
/// GetEventDataStore on an event data store to check whether the event data store logs
/// management events").
fn store_collecting(id: &str, region: &str, categories: &[&str]) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status: EventDataStoreState::Enabled,
        advanced_event_selectors: vec![AdvancedEventSelector {
            name: Some("categories".to_string()),
            field_selectors: vec![AdvancedFieldSelector {
                field: EVENT_CATEGORY_FIELD.to_string(),
                equals: Some(categories.iter().map(|c| (*c).to_string()).collect()),
                ..AdvancedFieldSelector::default()
            }],
        }],
        ..EventDataStoreRecord::default()
    }
}

/// Persist `record` in the registry every event data store operation reads, as
/// `CreateEventDataStore` will once it is implemented.
fn seed(emulator: &Emulator, record: EventDataStoreRecord) {
    emulator
        .state()
        .lock()
        .event_data_stores
        .insert(record.id.clone(), record);
}

/// Seed the pair the event-data-store cases address: a source that logs management
/// events and a destination that logs Insights events.
fn seed_lake_pair(emulator: &Emulator, region: &str) {
    seed(
        emulator,
        store_collecting(SOURCE_ID, region, &[DEFAULT_INSIGHTS_EVENT_CATEGORY]),
    );
    seed(
        emulator,
        store_collecting(DESTINATION_ID, region, &[INSIGHT_EVENT_CATEGORY]),
    );
}

/// The `InsightSelectors` a reply carries, or `Value::Null` for a reply without them.
fn selectors_of(body: &Value) -> Value {
    body.get("InsightSelectors").cloned().unwrap_or(Value::Null)
}

/// `GetTrail`'s `HasInsightSelectors` for `name`, which is derived from the stored
/// statement rather than tracked beside it.
async fn has_insight_selectors(emulator: &Emulator, name: &str) -> Value {
    let reply = emulator.call("GetTrail", &json!({"Name": name})).await;
    reply.expect_ok(&format!("GetTrail {name}"))["Trail"]["HasInsightSelectors"].clone()
}

/// A statement carrying every Insights type the model's enum declares, each on
/// `categories` (or with `EventCategories` left out when it is `None`).
fn statement(types: &[&str], categories: Option<&[&str]>) -> Value {
    Value::Array(
        types
            .iter()
            .map(|insight_type| {
                let mut selector = json!({"InsightType": insight_type});
                if let Some(categories) = categories {
                    selector["EventCategories"] = json!(categories);
                }
                selector
            })
            .collect(),
    )
}

/// What a stored statement reads back as: the types as given, each carrying the
/// categories it selects — the ones the request named, or the documented default.
fn stored(types: &[&str], categories: Option<&[&str]>) -> Value {
    let categories = categories.map_or_else(
        || vec![DEFAULT_INSIGHTS_EVENT_CATEGORY.to_string()],
        |named| named.iter().map(|c| (*c).to_string()).collect(),
    );
    Value::Array(
        types
            .iter()
            .map(|insight_type| json!({"InsightType": insight_type, "EventCategories": categories}))
            .collect(),
    )
}

/// A statement a trail accepts round-trips: the write answers it, the read answers it,
/// and `HasInsightSelectors` reads true.
///
/// The rows differ along what the rule generalises over: which Insights type (every
/// value of the model's `InsightType` enum), which event categories (absent, each value
/// of the model's `SourceEventCategory` enum, and both at once) and how many selectors
/// one statement carries.
#[tokio::test]
async fn a_statement_configured_on_a_trail_is_what_every_later_read_reports() {
    // (case, Insights types, EventCategories — `None` for a request that omits them)
    let mut rows: Vec<StatementRow<String>> = Vec::new();
    for &insight_type in INSIGHT_TYPE_VALUES {
        rows.push((
            format!("{insight_type}, no EventCategories"),
            vec![insight_type],
            None,
        ));
        // Every value of `SourceEventCategory`, read off the model rather than
        // written out.
        for &category in SOURCE_EVENT_CATEGORY_VALUES {
            rows.push((
                format!("{insight_type} on {category}"),
                vec![insight_type],
                Some(vec![category]),
            ));
        }
        rows.push((
            format!("{insight_type} on every category"),
            vec![insight_type],
            Some(SOURCE_EVENT_CATEGORY_VALUES.to_vec()),
        ));
    }
    rows.push((
        "every Insights type at once".to_string(),
        INSIGHT_TYPE_VALUES.to_vec(),
        Some(vec![DEFAULT_INSIGHTS_EVENT_CATEGORY]),
    ));

    for (case, types, categories) in rows {
        // A fresh emulator per row, so each row is read back on its own trail.
        let emulator = Emulator::new();
        let arn = create_trail(&emulator, HOME_REGION, "insights-trail").await;
        let expected = stored(&types, categories.as_deref());

        let mut request = json!({"TrailName": "insights-trail"});
        request["InsightSelectors"] = statement(&types, categories.as_deref());
        let put = emulator.call("PutInsightSelectors", &request).await;
        let body = put.expect_ok(&format!("PutInsightSelectors: {case}"));
        assert_eq!(
            (body.get("TrailARN"), selectors_of(body)),
            (Some(&json!(arn)), expected.clone()),
            "case {case}: expected the trail ARN and {expected}, got {put:?}"
        );

        // Read back through the implemented reader: the write is only real if a later
        // caller sees it.
        for identifier in [json!("insights-trail"), json!(arn)] {
            let got = emulator
                .call("GetInsightSelectors", &json!({"TrailName": identifier}))
                .await;
            let body = got.expect_ok(&format!("GetInsightSelectors by {identifier}: {case}"));
            assert_eq!(
                (body.get("TrailARN"), selectors_of(body)),
                (Some(&json!(arn)), expected.clone()),
                "case {case} by {identifier}: expected the trail ARN and {expected}, got {got:?}"
            );
        }

        assert_eq!(
            has_insight_selectors(&emulator, "insights-trail").await,
            json!(true),
            "case {case}: expected HasInsightSelectors true once a statement is configured"
        );
    }
}

/// "You also use PutInsightSelectors to turn off Insights event logging, by passing an
/// empty list of Insights types": the statement replaces whatever was configured
/// before, so an empty one leaves the resource with Insights off.
#[tokio::test]
async fn a_statement_replaces_the_previous_one_and_an_empty_one_turns_insights_off() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "replaced-trail").await;

    // Each row is applied in turn to the SAME trail, so a row that merged rather than
    // replaced would show up in the next read.
    let rows: Vec<StatementRow<&str>> = vec![
        ("both types on management events", INSIGHT_TYPE_VALUES.to_vec(), None),
        (
            "one type on data events",
            vec![INSIGHT_TYPE_VALUES[0]],
            Some(vec![DATA_INSIGHTS_EVENT_CATEGORY]),
        ),
        (
            "the other type on management events",
            vec![INSIGHT_TYPE_VALUES[1]],
            Some(vec![DEFAULT_INSIGHTS_EVENT_CATEGORY]),
        ),
    ];

    for (case, types, categories) in rows {
        let expected = stored(&types, categories.as_deref());
        let put = emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "TrailName": "replaced-trail",
                    "InsightSelectors": statement(&types, categories.as_deref()),
                }),
            )
            .await;
        assert_eq!(
            selectors_of(put.expect_ok(case)),
            expected,
            "case {case}: expected {expected}, got {put:?}"
        );
        let got = emulator
            .call(
                "GetInsightSelectors",
                &json!({"TrailName": "replaced-trail"}),
            )
            .await;
        assert_eq!(
            selectors_of(got.expect_ok(case)),
            expected,
            "case {case}: expected the read to report {expected}, got {got:?}"
        );
    }

    // The empty statement: Insights event logging off.
    let off = emulator
        .call(
            "PutInsightSelectors",
            &json!({"TrailName": "replaced-trail", "InsightSelectors": []}),
        )
        .await;
    assert_eq!(
        selectors_of(off.expect_ok("an empty statement")),
        json!([]),
        "case an empty statement: expected an empty statement back, got {off:?}"
    );
    assert_eq!(
        has_insight_selectors(&emulator, "replaced-trail").await,
        json!(false),
        "case an empty statement: expected HasInsightSelectors false once Insights are off"
    );
    emulator
        .call(
            "GetInsightSelectors",
            &json!({"TrailName": "replaced-trail"}),
        )
        .await
        .expect_error(
            "reading a trail with Insights off",
            400,
            "InsightNotEnabledException",
        );
}

/// A trail that never had Insights enabled has none to report: "If you run
/// GetInsightSelectors on a trail or event data store that does not have Insights
/// events enabled, the operation throws the exception InsightNotEnabledException."
#[tokio::test]
async fn a_resource_with_insights_never_enabled_is_refused_by_the_read() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "never-enabled").await;
    seed_lake_pair(&emulator, HOME_REGION);

    // (case, the request the read is made with)
    let rows: Vec<(&str, Value)> = vec![
        ("a trail by name", json!({"TrailName": "never-enabled"})),
        (
            "an event data store by id",
            json!({"EventDataStore": SOURCE_ID}),
        ),
        (
            "an event data store by ARN",
            json!({"EventDataStore": event_data_store_arn(HOME_REGION, SOURCE_ID)}),
        ),
    ];

    for (case, request) in rows {
        emulator
            .call("GetInsightSelectors", &request)
            .await
            .expect_error(case, 400, "InsightNotEnabledException");
    }

    assert_eq!(
        has_insight_selectors(&emulator, "never-enabled").await,
        json!(false),
        "case a trail nothing configured: expected HasInsightSelectors false"
    );
}

/// "The specified InsightType in the InsightSelectors statement is not valid. Valid
/// values for InsightType are ApiCallRateInsight and ApiErrorRateInsight": a statement
/// naming a type outside the model's enum is refused with
/// `InvalidInsightSelectorsException`, and changes nothing.
#[tokio::test]
async fn a_statement_naming_an_unknown_insights_type_is_refused() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "refusal-trail").await;
    // The trail starts out with a statement of its own, so a refusal that wrongly
    // wrote would show up as a changed read.
    let configured = stored(&[INSIGHT_TYPE_VALUES[0]], None);
    emulator
        .call(
            "PutInsightSelectors",
            &json!({
                "TrailName": "refusal-trail",
                "InsightSelectors": statement(&[INSIGHT_TYPE_VALUES[0]], None),
            }),
        )
        .await
        .expect_ok("the statement the refusals must not change");

    // (case, the InsightSelectors statement, the value the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a type that is not an Insights type at all",
            json!([{"InsightType": "NotARealInsightType"}]),
            "NotARealInsightType",
        ),
        (
            "a valid type in the wrong case",
            json!([{"InsightType": "apicallrateinsight"}]),
            "apicallrateinsight",
        ),
        (
            "a valid type with trailing whitespace",
            json!([{"InsightType": "ApiCallRateInsight "}]),
            "ApiCallRateInsight ",
        ),
        (
            "an empty type",
            json!([{"InsightType": ""}]),
            "",
        ),
        (
            "an unknown type beside a valid one",
            json!([
                {"InsightType": INSIGHT_TYPE_VALUES[0]},
                {"InsightType": "ApiThroughputInsight"},
            ]),
            "ApiThroughputInsight",
        ),
    ];

    for (case, selectors, value) in rows {
        let reply = emulator
            .call(
                "PutInsightSelectors",
                &json!({"TrailName": "refusal-trail", "InsightSelectors": selectors}),
            )
            .await;
        reply.expect_error(case, 400, "InvalidInsightSelectorsException");
        // The modelled shape's own member is `Message`, which is what a client reads,
        // so the refusal is asserted on that member rather than through a reader that
        // would also accept the framework's lowercase `message`.
        assert_eq!(
            reply.body.get("Message"),
            Some(&json!(invalid_insight_type_message(value))),
            "case {case}: expected Message to name {value:?} and the value set, got {reply:?}"
        );
    }

    // A selector that states no type at all states none of the valid ones, so it is
    // the same refusal.
    let no_type = emulator
        .call(
            "PutInsightSelectors",
            &json!({"TrailName": "refusal-trail", "InsightSelectors": [{}]}),
        )
        .await;
    no_type.expect_error(
        "a selector stating no InsightType",
        400,
        "InvalidInsightSelectorsException",
    );
    assert_eq!(
        no_type.body.get("Message"),
        Some(&json!(invalid_insight_type_message(""))),
        "case a selector stating no InsightType: expected the refusal to name the value set, \
         got {no_type:?}"
    );

    // Nothing the refusals touched was written.
    let got = emulator
        .call("GetInsightSelectors", &json!({"TrailName": "refusal-trail"}))
        .await;
    assert_eq!(
        selectors_of(got.expect_ok("after every refusal")),
        configured,
        "case after every refusal: expected the configured statement {configured}, got {got:?}"
    );
}

/// `TrailName` and `EventDataStore` name resources of different kinds, and
/// `InsightsDestination` belongs to the event data store form, where it is required: a
/// request that does not address exactly one resource is
/// `InvalidParameterCombinationException`.
#[tokio::test]
async fn a_request_that_does_not_address_one_resource_is_refused() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "combination-trail").await;
    seed_lake_pair(&emulator, HOME_REGION);

    let selectors = statement(&[INSIGHT_TYPE_VALUES[0]], None);
    let destination = event_data_store_arn(HOME_REGION, DESTINATION_ID);

    // (case, the identifier members the write carries)
    let write_rows: Vec<(&str, Value)> = vec![
        ("neither a trail nor an event data store", json!({})),
        (
            "both a trail and an event data store",
            json!({"TrailName": "combination-trail", "EventDataStore": SOURCE_ID}),
        ),
        (
            "a trail with an Insights destination",
            json!({"TrailName": "combination-trail", "InsightsDestination": destination}),
        ),
        (
            "an event data store with no Insights destination",
            json!({"EventDataStore": SOURCE_ID}),
        ),
        (
            "an Insights destination on its own",
            json!({"InsightsDestination": destination}),
        ),
    ];

    for (case, identifiers) in write_rows {
        let mut request = identifiers;
        request["InsightSelectors"] = selectors.clone();
        emulator
            .call("PutInsightSelectors", &request)
            .await
            .expect_error(
                &format!("PutInsightSelectors: {case}"),
                400,
                "InvalidParameterCombinationException",
            );
    }

    // The read carries only the pair, so its rule is the pair rule alone.
    let read_rows: Vec<(&str, Value)> = vec![
        ("neither a trail nor an event data store", json!({})),
        (
            "both a trail and an event data store",
            json!({"TrailName": "combination-trail", "EventDataStore": SOURCE_ID}),
        ),
    ];

    for (case, request) in read_rows {
        emulator
            .call("GetInsightSelectors", &request)
            .await
            .expect_error(
                &format!("GetInsightSelectors: {case}"),
                400,
                "InvalidParameterCombinationException",
            );
    }
}

/// Both operations read a trail identifier as a bare name or a trail ARN, and refuse a
/// malformed one as the form it was written in; a well-formed one naming no trail is
/// `TrailNotFoundException`.
#[tokio::test]
async fn a_malformed_or_unknown_trail_identifier_is_refused() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "known-trail").await;

    // (case, TrailName, expected HTTP status, expected error code)
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "an ARN with no trail resource",
            "arn:aws:cloudtrail:us-west-2:000000000000:trail",
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:us-east-1:000000000000:trail/known-trail",
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "an ARN whose trail name breaks the name rules",
            "arn:aws:cloudtrail:us-east-1:000000000000:trail/-nope-",
            400,
            "CloudTrailARNInvalidException",
        ),
        (
            "a name that is too short",
            "ab",
            400,
            "InvalidTrailNameException",
        ),
        (
            "a name with adjacent punctuation",
            "my--namespace",
            400,
            "InvalidTrailNameException",
        ),
        (
            "a name in IP address format",
            "192.168.5.4",
            400,
            "InvalidTrailNameException",
        ),
        (
            "a well-formed name naming no trail",
            "absent-trail",
            400,
            "TrailNotFoundException",
        ),
        (
            "a well-formed ARN naming no trail",
            "arn:aws:cloudtrail:us-east-1:000000000000:trail/absent-trail",
            400,
            "TrailNotFoundException",
        ),
    ];

    for (case, trail_name, status, code) in rows {
        emulator
            .call("GetInsightSelectors", &json!({"TrailName": trail_name}))
            .await
            .expect_error(&format!("GetInsightSelectors: {case}"), status, code);
        emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "TrailName": trail_name,
                    "InsightSelectors": statement(&[INSIGHT_TYPE_VALUES[0]], None),
                }),
            )
            .await
            .expect_error(&format!("PutInsightSelectors: {case}"), status, code);
    }
}

/// Insights selectors are configured from the Region the trail was created in, as every
/// other change to a trail is; the read carries no such rule, so the same identifier
/// answers the configured statement from either Region.
#[tokio::test]
async fn the_write_runs_only_in_the_trail_home_region() {
    let emulator = Emulator::new();
    // A multi-Region trail, so its shadow copy is visible — and still not writable —
    // from another Region.
    let created = emulator
        .call(
            "CreateTrail",
            &json!({
                "Name": "home-bound",
                "S3BucketName": "bucket-for-home-bound",
                "IsMultiRegionTrail": true,
            }),
        )
        .await;
    let arn = created.expect_ok("CreateTrail home-bound")["TrailARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let selectors = statement(&[INSIGHT_TYPE_VALUES[0]], None);
    emulator
        .call(
            "PutInsightSelectors",
            &json!({"TrailName": "home-bound", "InsightSelectors": selectors.clone()}),
        )
        .await
        .expect_ok("the statement configured in the home Region");

    // (case, TrailName as written)
    let rows: Vec<(&str, Value)> = vec![
        ("a bare name from another Region", json!("home-bound")),
        ("the home-Region ARN from another Region", json!(arn)),
    ];

    for (case, trail_name) in rows {
        emulator
            .call_in(
                "PutInsightSelectors",
                OTHER_REGION,
                &json!({"TrailName": trail_name, "InsightSelectors": selectors.clone()}),
            )
            .await
            .expect_error(
                &format!("PutInsightSelectors: {case}"),
                400,
                "InvalidHomeRegionException",
            );
    }

    // The read answers the configured statement from either Region, by ARN.
    let expected = stored(&[INSIGHT_TYPE_VALUES[0]], None);
    for region in [HOME_REGION, OTHER_REGION] {
        let got = emulator
            .call_in("GetInsightSelectors", region, &json!({"TrailName": arn}))
            .await;
        assert_eq!(
            selectors_of(got.expect_ok(region)),
            expected,
            "case GetInsightSelectors from {region}: expected {expected}, got {got:?}"
        );
    }
}

/// An event data store's statement round-trips, and carries the destination store its
/// Insights events are logged to: "you must specify the ARNs (or ID suffix of the ARNs)
/// for the source event data store (EventDataStore) and the destination event data
/// store (InsightsDestination)."
#[tokio::test]
async fn a_statement_configured_on_an_event_data_store_names_its_destination() {
    let source_arn = event_data_store_arn(HOME_REGION, SOURCE_ID);
    let destination_arn = event_data_store_arn(HOME_REGION, DESTINATION_ID);

    // (case, how the source is addressed, how the destination is addressed)
    let rows: Vec<(&str, Value, Value)> = vec![
        ("both by bare id", json!(SOURCE_ID), json!(DESTINATION_ID)),
        (
            "both by ARN",
            json!(source_arn.clone()),
            json!(destination_arn.clone()),
        ),
        (
            "the source by ARN, the destination by id",
            json!(source_arn.clone()),
            json!(DESTINATION_ID),
        ),
    ];

    for (case, source, destination) in rows {
        // A fresh emulator per row, so each row configures its own pair.
        let emulator = Emulator::new();
        seed_lake_pair(&emulator, HOME_REGION);
        let expected = stored(&[INSIGHT_TYPE_VALUES[0]], None);

        let put = emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "EventDataStore": source,
                    "InsightsDestination": destination,
                    "InsightSelectors": statement(&[INSIGHT_TYPE_VALUES[0]], None),
                }),
            )
            .await;
        let body = put.expect_ok(case);
        assert_eq!(
            (
                body.get("EventDataStoreArn"),
                body.get("InsightsDestination"),
                body.get("TrailARN"),
                selectors_of(body),
            ),
            (
                Some(&json!(source_arn)),
                Some(&json!(destination_arn)),
                None,
                expected.clone(),
            ),
            "case {case}: expected the source and destination ARNs and {expected}, got {put:?}"
        );

        // Read back through the implemented reader, by each form of the identifier.
        for identifier in [json!(SOURCE_ID), json!(source_arn.clone())] {
            let got = emulator
                .call(
                    "GetInsightSelectors",
                    &json!({"EventDataStore": identifier}),
                )
                .await;
            let body = got.expect_ok(&format!("{case}, read by {identifier}"));
            assert_eq!(
                (
                    body.get("EventDataStoreArn"),
                    body.get("InsightsDestination"),
                    selectors_of(body),
                ),
                (
                    Some(&json!(source_arn)),
                    Some(&json!(destination_arn)),
                    expected.clone(),
                ),
                "case {case} read by {identifier}: expected the stored statement, got {got:?}"
            );
        }
    }
}

/// What an event data store's statement is refused for: a category Insights do not
/// support there, a source that does not log management events, a destination that does
/// not log Insights events, and an identifier that names no store.
#[tokio::test]
async fn an_event_data_store_statement_outside_the_documented_rules_is_refused() {
    let destination_arn = event_data_store_arn(HOME_REGION, DESTINATION_ID);
    let absent_id = "33333333-3333-4333-8333-333333333333";
    let enabling = statement(&[INSIGHT_TYPE_VALUES[0]], None);

    // (case, the request's EventDataStore, its InsightsDestination, the statement, the
    //  expected error code)
    let rows: Vec<(&str, Value, Value, Value, &str)> = vec![
        (
            "Insights on the store's data events",
            json!(SOURCE_ID),
            json!(DESTINATION_ID),
            statement(&[INSIGHT_TYPE_VALUES[0]], Some(&[DATA_INSIGHTS_EVENT_CATEGORY])),
            "InvalidInsightSelectorsException",
        ),
        (
            "a source that does not log management events",
            json!(DESTINATION_ID),
            json!(DESTINATION_ID),
            enabling.clone(),
            "InvalidInsightSelectorsException",
        ),
        (
            "a destination that does not log Insights events",
            json!(SOURCE_ID),
            json!(SOURCE_ID),
            enabling.clone(),
            "InvalidInsightSelectorsException",
        ),
        (
            "a source naming no store",
            json!(absent_id),
            json!(DESTINATION_ID),
            enabling.clone(),
            "InvalidParameterException",
        ),
        (
            "a destination naming no store",
            json!(SOURCE_ID),
            json!(absent_id),
            enabling.clone(),
            "InvalidParameterException",
        ),
        (
            "a source in another Region",
            json!(event_data_store_arn(OTHER_REGION, SOURCE_ID)),
            json!(DESTINATION_ID),
            enabling.clone(),
            "InvalidParameterException",
        ),
        (
            "an identifier that is neither an ARN nor an id",
            json!("arn:aws:s3:::not-a-store"),
            json!(DESTINATION_ID),
            enabling.clone(),
            "CloudTrailARNInvalidException",
        ),
    ];

    for (case, source, destination, selectors, code) in rows {
        let emulator = Emulator::new();
        seed_lake_pair(&emulator, HOME_REGION);
        emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "EventDataStore": source,
                    "InsightsDestination": destination,
                    "InsightSelectors": selectors,
                }),
            )
            .await
            .expect_error(case, 400, code);

        // A refusal persists nothing: the source still has Insights off.
        emulator
            .call("GetInsightSelectors", &json!({"EventDataStore": SOURCE_ID}))
            .await
            .expect_error(
                &format!("{case}, after the refusal"),
                400,
                "InsightNotEnabledException",
            );
    }

    // Turning Insights OFF is not held to the enabling prerequisites: the destination
    // is only needed to log Insights events, and an empty statement logs none.
    let emulator = Emulator::new();
    seed_lake_pair(&emulator, HOME_REGION);
    let off = emulator
        .call(
            "PutInsightSelectors",
            &json!({
                "EventDataStore": SOURCE_ID,
                "InsightsDestination": destination_arn,
                "InsightSelectors": [],
            }),
        )
        .await;
    assert_eq!(
        selectors_of(off.expect_ok("an empty statement on a store")),
        json!([]),
        "case an empty statement on a store: expected an empty statement back, got {off:?}"
    );
}

/// Reading changes nothing a later caller sees: the same read answers the same values,
/// and the registry it computed them from is untouched.
#[tokio::test]
async fn reading_insight_selectors_changes_nothing() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "stable-trail").await;
    seed_lake_pair(&emulator, HOME_REGION);
    let selectors = statement(INSIGHT_TYPE_VALUES, Some(&[DEFAULT_INSIGHTS_EVENT_CATEGORY]));
    emulator
        .call(
            "PutInsightSelectors",
            &json!({"TrailName": "stable-trail", "InsightSelectors": selectors.clone()}),
        )
        .await
        .expect_ok("the trail's statement");
    emulator
        .call(
            "PutInsightSelectors",
            &json!({
                "EventDataStore": SOURCE_ID,
                "InsightsDestination": DESTINATION_ID,
                "InsightSelectors": statement(&[INSIGHT_TYPE_VALUES[0]], None),
            }),
        )
        .await
        .expect_ok("the store's statement");

    let before = format!("{:?}", emulator.state().lock().trails);
    let stores_before = format!("{:?}", emulator.state().lock().event_data_stores);

    // (case, the request the read is made with)
    let rows: Vec<(&str, Value)> = vec![
        ("a trail", json!({"TrailName": "stable-trail"})),
        ("an event data store", json!({"EventDataStore": SOURCE_ID})),
    ];

    for (case, request) in rows {
        let first = emulator.call("GetInsightSelectors", &request).await;
        let second = emulator.call("GetInsightSelectors", &request).await;
        assert_eq!(
            first.expect_ok(case),
            second.expect_ok(case),
            "case {case}: expected two reads to answer the same values, got {first:?} then \
             {second:?}"
        );
    }

    assert_eq!(
        (
            format!("{:?}", emulator.state().lock().trails),
            format!("{:?}", emulator.state().lock().event_data_stores),
        ),
        (before, stores_before),
        "case after every read: expected the registries to be untouched"
    );
}
