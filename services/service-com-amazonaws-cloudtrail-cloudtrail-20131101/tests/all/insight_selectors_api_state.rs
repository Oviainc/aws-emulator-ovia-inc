// SPDX-License-Identifier: CC0-1.0

//! `GetInsightSelectors` over state built ONLY through this service's own API.
//!
//! The rule under test: `GetInsightSelectors` reports the Insights configuration
//! PERSISTED on the one resource a request addresses — the statement the last
//! `PutInsightSelectors` wrote, reported the same through either documented
//! identifier form — and a request that addresses a resource carrying no statement,
//! that addresses no single resource, or that names an identifier the service cannot
//! read, is refused with the status, the error code and the wording that refusal
//! class carries.
//!
//! The response members and the refusals are the model's
//! (`smithy_query(query="operation", operation="GetInsightSelectors")`): the answer
//! carries `TrailARN`, `InsightSelectors`, `EventDataStoreArn` and
//! `InsightsDestination`, and the operation binds `InsightNotEnabledException`,
//! `TrailNotFoundException`, `CloudTrailARNInvalidException`,
//! `InvalidTrailNameException`, `InvalidParameterException` and
//! `InvalidParameterCombinationException`.
//!
//! What this suite adds to [`super::insight_selectors`] — which seeds the event data
//! store registry through the `state` module, because `CreateEventDataStore` was a
//! scaffold stub when it was written — is that EVERY resource read here is made over
//! the wire, with `CreateTrail` and `CreateEventDataStore`. So what is asserted is
//! what a client that only ever called this API sees: the reported configuration is
//! computed from state the API itself built, not from anything a test wrote into the
//! state struct.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::INSIGHT_TYPE_VALUES;

use super::emulator::{Emulator, HOME_REGION};

/// A Region the cases use as "not where the call was made".
const OTHER_REGION: &str = "eu-west-1";

/// The event category a statement that names no `EventCategories` selects: "If
/// EventCategories is not provided, the specified Insights types are enabled on
/// management API calls by default" (model doc for
/// `PutInsightSelectorsRequest$InsightSelectors`).
const DEFAULT_CATEGORY: &str = "Management";

/// The other category Insights support on a trail, from the model's
/// `SourceEventCategory` enum (`ops::types::SOURCE_EVENT_CATEGORY_VALUES`).
const DATA_CATEGORY: &str = "Data";

/// The event category an event data store that LOGS Insights events collects — what
/// "the destination event data store … must log Insights events" (model doc for
/// `InvalidInsightSelectorsException`) is read against.
const INSIGHT_CATEGORY: &str = "Insight";

/// A well-formed store id of the form `CreateEventDataStore` mints, that no case
/// creates.
const ABSENT_STORE_ID: &str = "33333333-3333-4333-8333-333333333333";

/// A string member of a reply body, or `""` when it carries none.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create a trail over the wire and hand back the ARN the service minted for it.
async fn create_trail(emulator: &Emulator, name: &str) -> String {
    let request = json!({"Name": name, "S3BucketName": format!("bucket-for-{name}")});
    let reply = emulator.call("CreateTrail", &request).await;
    text(reply.expect_ok(&format!("CreateTrail {name}")), "TrailARN")
}

/// Create an event data store over the wire that collects `category`, and hand back
/// the ARN the service minted for it.
///
/// Which categories a store collects is what its advanced event selectors state on
/// the `eventCategory` field, so that is how a source store that logs management
/// events and a destination store that logs Insights events are made.
async fn create_store(emulator: &Emulator, region: &str, name: &str, category: &str) -> String {
    let request = json!({
        "Name": name,
        "AdvancedEventSelectors": [{
            "Name": format!("{category} events"),
            "FieldSelectors": [{"Field": "eventCategory", "Equals": [category]}],
        }],
    });
    let reply = emulator
        .call_in("CreateEventDataStore", region, &request)
        .await;
    text(
        reply.expect_ok(&format!("CreateEventDataStore {name} in {region}")),
        "EventDataStoreArn",
    )
}

/// The id suffix of a store ARN — the other identifier form the model documents
/// ("the ARN (or ID suffix of the ARN)").
fn id_of(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// The `InsightSelectors` statement a request sends: one selector per Insights type,
/// each naming `categories` when the case supplies them and leaving the member out
/// when it does not.
fn statement(types: &[&str], categories: Option<&[&str]>) -> Value {
    Value::Array(
        types
            .iter()
            .map(|insight_type| match categories {
                Some(categories) => {
                    json!({"InsightType": insight_type, "EventCategories": categories})
                }
                None => json!({"InsightType": insight_type}),
            })
            .collect(),
    )
}

/// The statement a read must report: every member filled in, so a request that left
/// `EventCategories` out still reports the category it selected.
fn reported(types: &[&str], categories: &[&str]) -> Value {
    Value::Array(
        types
            .iter()
            .map(|insight_type| {
                json!({"InsightType": insight_type, "EventCategories": categories})
            })
            .collect(),
    )
}

/// One row of the trail statement table: the case, the Insights types the statement
/// names, the `EventCategories` it names (`None` for a request that leaves the member
/// out), and the categories the report must carry.
type StatementRow = (
    &'static str,
    Vec<&'static str>,
    Option<Vec<&'static str>>,
    Vec<&'static str>,
);

/// A statement configured on a trail is what the read reports, for every form the
/// identifier may take. The rows differ in how many Insights types the statement
/// names, whether it names `EventCategories` at all, and which categories it selects.
#[tokio::test]
async fn a_trail_statement_is_reported_from_state_through_either_identifier_form() {
    let emulator = Emulator::new();

    // (case, the Insights types the statement names, the categories it names — `None`
    // for a request that leaves the member out, the categories the report must carry)
    let rows: Vec<StatementRow> = vec![
        (
            "one type, no EventCategories",
            vec![INSIGHT_TYPE_VALUES[0]],
            None,
            vec![DEFAULT_CATEGORY],
        ),
        (
            "one type, management events named",
            vec![INSIGHT_TYPE_VALUES[1]],
            Some(vec![DEFAULT_CATEGORY]),
            vec![DEFAULT_CATEGORY],
        ),
        (
            "one type, data events named",
            vec![INSIGHT_TYPE_VALUES[0]],
            Some(vec![DATA_CATEGORY]),
            vec![DATA_CATEGORY],
        ),
        (
            "every modelled type, both categories",
            INSIGHT_TYPE_VALUES.to_vec(),
            Some(vec![DEFAULT_CATEGORY, DATA_CATEGORY]),
            vec![DEFAULT_CATEGORY, DATA_CATEGORY],
        ),
        (
            "every modelled type, no EventCategories",
            INSIGHT_TYPE_VALUES.to_vec(),
            None,
            vec![DEFAULT_CATEGORY],
        ),
    ];

    for (index, (case, types, categories, expected_categories)) in rows.into_iter().enumerate() {
        let name = format!("insights-trail-{index}");
        let arn = create_trail(&emulator, &name).await;
        emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "TrailName": name,
                    "InsightSelectors": statement(&types, categories.as_deref()),
                }),
            )
            .await
            .expect_ok(&format!("{case}: PutInsightSelectors"));

        let expected = reported(&types, &expected_categories);
        // A bare name names the trail in the Region the call was made in; an ARN
        // names the Region it carries, and the read — unlike the write — carries no
        // home-Region rule, so the same ARN answers from another Region too.
        for (form, region, identifier) in [
            ("by bare name", HOME_REGION, name.clone()),
            ("by ARN", HOME_REGION, arn.clone()),
            ("by ARN from another Region", OTHER_REGION, arn.clone()),
        ] {
            let case = format!("{case}, read {form}");
            let reply = emulator
                .call_in(
                    "GetInsightSelectors",
                    region,
                    &json!({"TrailName": identifier}),
                )
                .await;
            let body = reply.expect_ok(&case).clone();
            assert_eq!(
                body.get("InsightSelectors"),
                Some(&expected),
                "case {case}: expected the stored statement {expected}, got {body:?}"
            );
            assert_eq!(
                text(&body, "TrailARN"),
                arn,
                "case {case}: expected the stored trail ARN {arn}, got {body:?}"
            );
            assert_eq!(
                (
                    body.get("EventDataStoreArn"),
                    body.get("InsightsDestination")
                ),
                (None, None),
                "case {case}: both event data store members belong to the store form, \
                 got {body:?}"
            );
        }
    }
}

/// A statement configured on an event data store is reported with the source store's
/// own ARN and the destination store its Insights events are logged to, whichever
/// identifier form the write and the read used.
#[tokio::test]
async fn an_event_data_store_statement_is_reported_with_the_destination_it_logs_to() {
    let emulator = Emulator::new();
    // The pair Insights on a store needs: a source that logs management events and a
    // destination that logs Insights events — both minted over the wire.
    let source = create_store(&emulator, HOME_REGION, "insights-source", DEFAULT_CATEGORY).await;
    let destination =
        create_store(&emulator, HOME_REGION, "insights-destination", INSIGHT_CATEGORY).await;
    let source_id = id_of(&source);
    let destination_id = id_of(&destination);
    let expected = reported(&[INSIGHT_TYPE_VALUES[0]], &[DEFAULT_CATEGORY]);

    // (case, the identifier the write names the source by, the one it names the
    // destination by)
    for (case, written_source, written_destination) in [
        ("both by ARN", source.clone(), destination.clone()),
        ("both by id suffix", source_id.clone(), destination_id.clone()),
        (
            "source by id suffix, destination by ARN",
            source_id.clone(),
            destination.clone(),
        ),
        (
            "source by ARN, destination by id suffix",
            source.clone(),
            destination_id.clone(),
        ),
    ] {
        emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "EventDataStore": written_source,
                    "InsightsDestination": written_destination,
                    "InsightSelectors": statement(&[INSIGHT_TYPE_VALUES[0]], None),
                }),
            )
            .await
            .expect_ok(&format!("{case}: PutInsightSelectors"));

        for (form, identifier) in [("ARN", source.clone()), ("id suffix", source_id.clone())] {
            let case = format!("{case}, read by {form}");
            let reply = emulator
                .call(
                    "GetInsightSelectors",
                    &json!({"EventDataStore": identifier}),
                )
                .await;
            let body = reply.expect_ok(&case).clone();
            assert_eq!(
                body.get("InsightSelectors"),
                Some(&expected),
                "case {case}: expected the stored statement {expected}, got {body:?}"
            );
            assert_eq!(
                text(&body, "EventDataStoreArn"),
                source,
                "case {case}: expected the source store ARN {source}, got {body:?}"
            );
            assert_eq!(
                text(&body, "InsightsDestination"),
                destination,
                "case {case}: expected the destination store ARN {destination}, got {body:?}"
            );
            assert_eq!(
                body.get("TrailARN"),
                None,
                "case {case}: TrailARN belongs to the trail form, got {body:?}"
            );
        }
    }

    // An empty statement turns Insights event logging off, and the read then refuses
    // the store it was just reporting on.
    emulator
        .call(
            "PutInsightSelectors",
            &json!({
                "EventDataStore": source,
                "InsightsDestination": destination,
                "InsightSelectors": [],
            }),
        )
        .await
        .expect_ok("turning Insights off on the source store");
    let case = "a store whose Insights were turned off again";
    let reply = emulator
        .call("GetInsightSelectors", &json!({"EventDataStore": source}))
        .await;
    reply.expect_error(case, 400, "InsightNotEnabledException");
    let expected_message = format!(
        "Event data store {source} does not have Insights enabled. Edit the trail settings \
         to enable Insights, and then try the operation again."
    );
    assert_eq!(
        reply.message(),
        expected_message,
        "case {case}: expected {expected_message:?}, got {:?}",
        reply.message()
    );
}

/// Every refusal of the read carries the status, the error code and the wording of
/// its class, with the identifier the request sent or the ARN it resolved to in it.
/// The rows differ in the resource kind, the identifier form and which rule the
/// request broke.
#[tokio::test]
async fn every_refusal_of_the_read_carries_the_wording_of_its_class() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    // A trail that exists and has never had Insights enabled, and a store that lives
    // in another Region — both made over the wire.
    let never_enabled = create_trail(&emulator, "insights-never-enabled").await;
    let elsewhere = create_store(&emulator, OTHER_REGION, "store-elsewhere", DEFAULT_CATEGORY).await;
    let elsewhere_id = id_of(&elsewhere);

    let absent_trail_here = format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/absent-trail");
    let absent_trail_there =
        format!("arn:aws:cloudtrail:{OTHER_REGION}:{account}:trail/absent-trail");
    let not_a_trail_arn = format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:badtrail");
    let another_services_arn = format!("arn:aws:s3:{HOME_REGION}:{account}:trail/absent-trail");
    let absent_store_arn =
        format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{ABSENT_STORE_ID}");
    let elsewhere_store_here =
        format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{elsewhere_id}");
    let bad_suffix_store_arn =
        format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/not-a-store-id");

    // (case, the request, the HTTP status, the error code, the message)
    let rows: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "a bare trail name naming no trail",
            json!({"TrailName": "absent-trail"}),
            400,
            "TrailNotFoundException",
            format!("Unknown trail: absent-trail for the user: {account}"),
        ),
        (
            "a trail ARN naming no trail",
            json!({"TrailName": absent_trail_here.clone()}),
            400,
            "TrailNotFoundException",
            format!("Unknown trail: {absent_trail_here} for the user: {account}"),
        ),
        (
            "a trail ARN of another Region naming no trail",
            json!({"TrailName": absent_trail_there.clone()}),
            400,
            "TrailNotFoundException",
            format!("Unknown trail: {absent_trail_there} for the user: {account}"),
        ),
        (
            "a trail that exists with Insights never enabled",
            json!({"TrailName": "insights-never-enabled"}),
            400,
            "InsightNotEnabledException",
            format!(
                "Trail {never_enabled} does not have Insights enabled. Edit the trail \
                 settings to enable Insights, and then try the operation again."
            ),
        ),
        (
            "an ARN whose resource is not a trail",
            json!({"TrailName": not_a_trail_arn.clone()}),
            400,
            "CloudTrailARNInvalidException",
            format!("The ARN {not_a_trail_arn} is invalid."),
        ),
        (
            "an ARN of another service",
            json!({"TrailName": another_services_arn.clone()}),
            400,
            "CloudTrailARNInvalidException",
            format!("The ARN {another_services_arn} is invalid."),
        ),
        (
            "a trail name in IP address format",
            json!({"TrailName": "192.168.5.4"}),
            400,
            "InvalidTrailNameException",
            "Trail names must not be formatted as an IP address.".to_string(),
        ),
        (
            "a trail name below the documented minimum length",
            json!({"TrailName": "ab"}),
            400,
            "InvalidTrailNameException",
            "Trail name too short. Minimum allowed length: 3 characters. Specified name \
             length: 2 characters."
                .to_string(),
        ),
        (
            "a request naming neither resource",
            json!({}),
            400,
            "InvalidParameterCombinationException",
            "You must specify either a trail or an event data store.".to_string(),
        ),
        (
            "a request naming both resources",
            json!({"TrailName": "insights-never-enabled", "EventDataStore": ABSENT_STORE_ID}),
            400,
            "InvalidParameterCombinationException",
            "You must specify either a trail or an event data store.".to_string(),
        ),
        (
            "a store id naming no store",
            json!({"EventDataStore": ABSENT_STORE_ID}),
            400,
            "InvalidParameterException",
            format!("EventDataStore {absent_store_arn} does not name an event data store."),
        ),
        (
            "a store ARN naming no store",
            json!({"EventDataStore": absent_store_arn.clone()}),
            400,
            "InvalidParameterException",
            format!("EventDataStore {absent_store_arn} does not name an event data store."),
        ),
        (
            "a store of another Region named by its bare id",
            json!({"EventDataStore": elsewhere_id.clone()}),
            400,
            "InvalidParameterException",
            format!("EventDataStore {elsewhere_store_here} does not name an event data store."),
        ),
        (
            // An ARN's resource suffix IS the store the ARN names, so for that form
            // whether a store of that id exists is the next question and not a
            // question about the ARN's form.
            "a store ARN whose resource suffix names no store",
            json!({"EventDataStore": bad_suffix_store_arn.clone()}),
            400,
            "InvalidParameterException",
            format!("EventDataStore {bad_suffix_store_arn} does not name an event data store."),
        ),
        (
            "a store identifier that is neither an ARN nor an id this service mints",
            json!({"EventDataStore": "not-a-store-id"}),
            400,
            "CloudTrailARNInvalidException",
            format!("The event data store ARN {bad_suffix_store_arn} format is not valid."),
        ),
    ];

    for (case, request, status, code, message) in rows {
        let reply = emulator.call("GetInsightSelectors", &request).await;
        reply.expect_error(case, status, code);
        assert_eq!(
            reply.message(),
            message,
            "case {case}: expected {message:?}, got {:?}",
            reply.message()
        );
    }

    // The store in the other Region is still readable where it lives, so the refusals
    // above were about the Region a bare id addresses and not about the store.
    emulator
        .call_in(
            "GetInsightSelectors",
            OTHER_REGION,
            &json!({"EventDataStore": elsewhere_id}),
        )
        .await
        .expect_error(
            "the store in its own Region, with Insights never enabled",
            400,
            "InsightNotEnabledException",
        );
}
