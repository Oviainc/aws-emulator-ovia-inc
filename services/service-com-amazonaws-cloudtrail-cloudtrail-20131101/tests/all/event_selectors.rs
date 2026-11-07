// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetEventSelectors` / `PutEventSelectors`.
//!
//! Every case goes over the wire through the shared harness in
//! [`super::emulator`] — an `awsJson1_1` `POST /` with this service's own
//! `X-Amz-Target` prefix, dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted here is what a client sees, including the HTTP
//! status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `GetEventSelectors` reports the ARN of the trail its identifier names plus the
//!   selectors configured on it: the advanced ones when advanced selectors are
//!   configured, otherwise the basic ones — and a trail nothing has configured logs by
//!   the documented default selector (all read and write management events, no data
//!   events);
//! * `PutEventSelectors` stores either the basic or the advanced selectors, never both,
//!   replacing whichever kind was configured before, and fills in each member's
//!   documented default;
//! * a selector set outside the documented limits — or one that would log NOTHING — is
//!   refused with `InvalidEventSelectorsException` and changes nothing;
//! * both operations read a trail identifier as a bare name or a trail ARN and refuse a
//!   malformed one as the form it was written in, while `PutEventSelectors` — unlike the
//!   read — runs only in the trail's home Region;
//! * the selectors are configured on the ONE trail the identifier resolves to — every
//!   other trail keeps what it carried — and re-applying a selector set changes
//!   nothing (`PutEventSelectors` is idempotent).
//!
//! How an `X-Amz-Target` names an operation is pinned by this crate's
//! `target_prefix_dispatch` / `target_routing` suites, not here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::READ_WRITE_TYPE_VALUES;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT, BASIC_DATA_RESOURCE_TYPES,
    DATA_RESOURCES_PER_TRAIL_LIMIT, EVENT_SELECTORS_PER_TRAIL_LIMIT,
    EXCLUDABLE_MANAGEMENT_EVENT_SOURCES,
};

// The emulator-under-test harness is shared with this crate's other suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, HOME_REGION};

/// A Region that is nobody's home Region in these cases.
const OTHER_REGION: &str = "eu-west-1";

/// The two members of a reply that carry selectors: `(EventSelectors,
/// AdvancedEventSelectors)`, with `Value::Null` for the one a reply leaves out.
fn selectors_of(body: &Value) -> (Value, Value) {
    (
        body.get("EventSelectors").cloned().unwrap_or(Value::Null),
        body.get("AdvancedEventSelectors")
            .cloned()
            .unwrap_or(Value::Null),
    )
}

/// The basic event selector a trail logs by until something configures it, from the
/// model doc for `EventSelector`: "trails created without specific event selectors will
/// be configured to log all read and write management events, and no data events".
fn default_selectors() -> Value {
    json!([{
        "ReadWriteType": "All",
        "IncludeManagementEvents": true,
        "DataResources": [],
        "ExcludeManagementEventSources": [],
    }])
}

/// Create a trail and hand back the ARN the service minted for it.
async fn create_trail(emulator: &Emulator, region: &str, request: &Value) -> String {
    let created = emulator.call_in("CreateTrail", region, request).await;
    created
        .expect_ok(&format!("CreateTrail {request}"))
        .get("TrailARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `n` basic event selectors spread over at most the documented maximum, carrying
/// `data_resources` `DataResource` entries in total.
///
/// A selector that carries no data resource INCLUDES management events: one that excludes
/// them and names no data resource would log nothing at all, which the service refuses, so
/// a generated selector logs management events exactly when it names no data resource.
fn selectors_with_data_resources(selectors: usize, data_resources: usize) -> Value {
    let rows: Vec<Value> = (0..selectors)
        .map(|selector| {
            let resources: Vec<Value> = (0..data_resources)
                .filter(|resource| resource % selectors == selector)
                .map(|resource| {
                    json!({
                        // Cycled over the basic resource-type catalog so the rows are
                        // not all one type.
                        "Type": BASIC_DATA_RESOURCE_TYPES[resource % BASIC_DATA_RESOURCE_TYPES.len()],
                        "Values": [format!("arn:aws:s3:::bucket-{resource}/")],
                    })
                })
                .collect();
            json!({
                "ReadWriteType": "All",
                "IncludeManagementEvents": resources.is_empty(),
                "DataResources": resources,
            })
        })
        .collect();
    Value::Array(rows)
}

/// One advanced event selector whose conditions carry `values` values in total, spread
/// over `fields` field selectors beside the mandatory `eventCategory` one.
///
/// Every advanced event selector states which category of events it selects, so that
/// field selector is always there and its one condition value is one of the `values`.
fn advanced_selector_with_values(fields: usize, values: usize) -> Value {
    let spread = values.saturating_sub(1);
    let mut field_selectors = vec![json!({"Field": "eventCategory", "Equals": ["Data"]})];
    field_selectors.extend((0..fields).map(|field| {
        let equals: Vec<String> = (0..spread)
            .filter(|value| value % fields == field)
            .map(|value| format!("arn:aws:s3:::bucket-{value}/"))
            .collect();
        json!({"Field": "resources.ARN", "StartsWith": equals})
    }));
    json!([{ "Name": "generated", "FieldSelectors": field_selectors }])
}

/// A trail nothing has configured reports the documented default selector, whichever
/// way its identifier addresses it and whichever Region the read is made from — reading
/// selectors does not depend on the caller's Region.
#[tokio::test]
async fn a_trail_nothing_configured_reports_the_default_event_selector() {
    let emulator = Emulator::new();
    let single = create_trail(
        &emulator,
        HOME_REGION,
        &json!({"Name": "single-region", "S3BucketName": "bucket-a"}),
    )
    .await;
    let multi = create_trail(
        &emulator,
        HOME_REGION,
        &json!({"Name": "multi.region", "S3BucketName": "bucket-b", "IsMultiRegionTrail": true}),
    )
    .await;
    let elsewhere = create_trail(
        &emulator,
        "us-west-2",
        &json!({"Name": "west-trail", "S3BucketName": "bucket-c"}),
    )
    .await;

    // (case, TrailName as the request writes it, the Region the read is made from,
    //  the trail ARN the reply must report)
    let rows: Vec<(&str, String, &str, &String)> = vec![
        (
            "a bare name in the trail's own Region",
            "single-region".to_string(),
            HOME_REGION,
            &single,
        ),
        (
            "the trail ARN in the trail's own Region",
            single.clone(),
            HOME_REGION,
            &single,
        ),
        (
            "the trail ARN from another Region",
            single.clone(),
            OTHER_REGION,
            &single,
        ),
        (
            "a multi-Region trail by bare name",
            "multi.region".to_string(),
            HOME_REGION,
            &multi,
        ),
        (
            "a trail homed in another Region, by bare name there",
            "west-trail".to_string(),
            "us-west-2",
            &elsewhere,
        ),
        (
            "a trail homed in another Region, by ARN from here",
            elsewhere.clone(),
            HOME_REGION,
            &elsewhere,
        ),
    ];

    for (case, trail_name, region, expected_arn) in rows {
        let reply = emulator
            .call_in("GetEventSelectors", region, &json!({"TrailName": trail_name}))
            .await;
        let body = reply.expect_ok(case).clone();
        assert_eq!(
            body["TrailARN"],
            json!(expected_arn),
            "case {case}: expected TrailARN {expected_arn}, got {body:?}"
        );
        assert_eq!(
            selectors_of(&body),
            (default_selectors(), Value::Null),
            "case {case}: expected the default event selector and no advanced \
             selectors, got {body:?}"
        );
    }

    // A read reports; it must not configure anything. `HasCustomEventSelectors` is
    // what says whether the trail carries selectors of its own.
    let trail = emulator
        .call("GetTrail", &json!({"Name": "single-region"}))
        .await;
    let trail = trail.expect_ok("GetTrail after the reads")["Trail"].clone();
    assert_eq!(
        trail["HasCustomEventSelectors"],
        json!(false),
        "case GetTrail after the reads: a read must not configure selectors, got {trail:?}"
    );
}

/// Whatever selector set `PutEventSelectors` is given, its reply and every later
/// `GetEventSelectors` report the stored set, with each member a request left out
/// filled in by its documented default. The rows differ in the KIND of selector
/// (basic vs advanced), the count, the `ReadWriteType`, which members are present, the
/// data-resource types and the excluded management event sources.
#[tokio::test]
async fn put_event_selectors_round_trips_through_get_event_selectors() {
    // (case, the selectors the request carries, the expected (EventSelectors,
    //  AdvancedEventSelectors) of every reply)
    let mut rows: Vec<(String, Value, (Value, Value))> = vec![
        (
            // An empty list is a configuration (nothing is logged), not an absent
            // member: it is stored and read back as the empty list, where a trail
            // nothing configured reads back the default selector. A request carrying
            // NEITHER member is refused by the case below.
            "an empty list of event selectors".to_string(),
            json!({"EventSelectors": []}),
            (json!([]), Value::Null),
        ),
        (
            "one selector carrying only ReadWriteType".to_string(),
            json!({"EventSelectors": [{"ReadWriteType": "ReadOnly"}]}),
            (
                json!([{
                    "ReadWriteType": "ReadOnly",
                    "IncludeManagementEvents": true,
                    "DataResources": [],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ),
        (
            // `IncludeManagementEvents` is carried WITH a data resource: a selector that
            // excludes management events and names no data resource logs nothing and is
            // refused (see `a_selector_set_outside_the_documented_rules_is_refused`).
            "one selector excluding management events for one data resource".to_string(),
            json!({"EventSelectors": [{
                "IncludeManagementEvents": false,
                "DataResources": [{
                    "Type": BASIC_DATA_RESOURCE_TYPES[0],
                    "Values": ["arn:aws:dynamodb:us-east-1:000000000000:table/orders"],
                }],
            }]}),
            (
                json!([{
                    "ReadWriteType": "All",
                    "IncludeManagementEvents": false,
                    "DataResources": [{
                        "Type": BASIC_DATA_RESOURCE_TYPES[0],
                        "Values": ["arn:aws:dynamodb:us-east-1:000000000000:table/orders"],
                    }],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ),
        (
            // `EXCLUDABLE_MANAGEMENT_EVENT_SOURCES` is the model's own list for
            // `EventSelector$ExcludeManagementEventSources`; the LAST entry is the
            // one the issue never cites.
            format!(
                "management events excluded by source {}",
                EXCLUDABLE_MANAGEMENT_EVENT_SOURCES[EXCLUDABLE_MANAGEMENT_EVENT_SOURCES.len() - 1]
            ),
            json!({"EventSelectors": [{
                "ExcludeManagementEventSources": [
                    EXCLUDABLE_MANAGEMENT_EVENT_SOURCES[EXCLUDABLE_MANAGEMENT_EVENT_SOURCES.len() - 1]
                ],
            }]}),
            (
                json!([{
                    "ReadWriteType": "All",
                    "IncludeManagementEvents": true,
                    "DataResources": [],
                    "ExcludeManagementEventSources": [
                        EXCLUDABLE_MANAGEMENT_EVENT_SOURCES[EXCLUDABLE_MANAGEMENT_EVENT_SOURCES.len() - 1]
                    ],
                }]),
                Value::Null,
            ),
        ),
        (
            "an advanced selector with a name and two field selectors".to_string(),
            json!({"AdvancedEventSelectors": [{
                "Name": "Log S3 data events",
                "FieldSelectors": [
                    {"Field": "eventCategory", "Equals": ["Data"]},
                    {"Field": "resources.type", "Equals": ["AWS::S3::Object"]},
                ],
            }]}),
            (
                Value::Null,
                json!([{
                    "Name": "Log S3 data events",
                    "FieldSelectors": [
                        {"Field": "eventCategory", "Equals": ["Data"]},
                        {"Field": "resources.type", "Equals": ["AWS::S3::Object"]},
                    ],
                }]),
            ),
        ),
        (
            "an advanced selector with no name and a negated condition".to_string(),
            json!({"AdvancedEventSelectors": [{
                "FieldSelectors": [
                    {"Field": "eventCategory", "Equals": ["Management"]},
                    {"Field": "eventSource", "NotEquals": ["kms.amazonaws.com"]},
                ],
            }]}),
            (
                Value::Null,
                json!([{
                    "FieldSelectors": [
                        {"Field": "eventCategory", "Equals": ["Management"]},
                        {"Field": "eventSource", "NotEquals": ["kms.amazonaws.com"]},
                    ],
                }]),
            ),
        ),
    ];

    // One row per `ReadWriteType` the model declares (`READ_WRITE_TYPE_VALUES` in
    // `ops::types`), so a member added to the model moves this table rather than
    // leaving a value untested.
    for read_write_type in READ_WRITE_TYPE_VALUES {
        rows.push((
            format!("a data-event selector of ReadWriteType {read_write_type}"),
            json!({"EventSelectors": [{
                "ReadWriteType": read_write_type,
                "IncludeManagementEvents": false,
                "DataResources": [{"Type": BASIC_DATA_RESOURCE_TYPES[0], "Values": ["arn:aws:dynamodb"]}],
            }]}),
            (
                json!([{
                    "ReadWriteType": read_write_type,
                    "IncludeManagementEvents": false,
                    "DataResources": [{"Type": BASIC_DATA_RESOURCE_TYPES[0], "Values": ["arn:aws:dynamodb"]}],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ));
    }

    // One row per basic data-resource type (`BASIC_DATA_RESOURCE_TYPES` in `state`,
    // the model's own list for `DataResource$Type`).
    for resource_type in BASIC_DATA_RESOURCE_TYPES {
        rows.push((
            format!("data events for {resource_type}"),
            json!({"EventSelectors": [{
                "IncludeManagementEvents": false,
                "DataResources": [{"Type": resource_type, "Values": ["arn:aws:s3:::amzn-s3-demo-bucket1/"]}],
            }]}),
            (
                json!([{
                    "ReadWriteType": "All",
                    "IncludeManagementEvents": false,
                    "DataResources": [{"Type": resource_type, "Values": ["arn:aws:s3:::amzn-s3-demo-bucket1/"]}],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ));
    }

    // The published bounds themselves: the maximum number of event selectors, the
    // maximum number of data resources across them, and the maximum number of
    // advanced condition values. One past each is refused by the case below.
    let at_the_selector_bound = selectors_with_data_resources(EVENT_SELECTORS_PER_TRAIL_LIMIT, 0);
    rows.push((
        format!("{EVENT_SELECTORS_PER_TRAIL_LIMIT} event selectors, the documented maximum"),
        json!({"EventSelectors": at_the_selector_bound.clone()}),
        (
            // Every generated selector already carries each member, so the stored set
            // is the set that was sent plus the empty `ExcludeManagementEventSources`.
            Value::Array(
                at_the_selector_bound
                    .as_array()
                    .expect("the generated selectors are an array")
                    .iter()
                    .map(|selector| {
                        let mut selector = selector.clone();
                        selector["ExcludeManagementEventSources"] = json!([]);
                        selector
                    })
                    .collect(),
            ),
            Value::Null,
        ),
    ));
    let at_the_data_resource_bound = selectors_with_data_resources(
        EVENT_SELECTORS_PER_TRAIL_LIMIT,
        DATA_RESOURCES_PER_TRAIL_LIMIT,
    );
    rows.push((
        format!("{DATA_RESOURCES_PER_TRAIL_LIMIT} data resources, the documented maximum"),
        json!({"EventSelectors": at_the_data_resource_bound.clone()}),
        (
            Value::Array(
                at_the_data_resource_bound
                    .as_array()
                    .expect("the generated selectors are an array")
                    .iter()
                    .map(|selector| {
                        let mut selector = selector.clone();
                        selector["ExcludeManagementEventSources"] = json!([]);
                        selector
                    })
                    .collect(),
            ),
            Value::Null,
        ),
    ));
    let at_the_value_bound =
        advanced_selector_with_values(5, ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT);
    rows.push((
        format!(
            "{ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT} advanced condition values, the \
             documented maximum"
        ),
        json!({"AdvancedEventSelectors": at_the_value_bound.clone()}),
        (Value::Null, at_the_value_bound),
    ));

    for (case, selectors, expected) in rows {
        // A fresh emulator per row, so each row is read back on its own trail.
        let emulator = Emulator::new();
        let arn = create_trail(
            &emulator,
            HOME_REGION,
            &json!({"Name": "selector-trail", "S3BucketName": "bucket-a"}),
        )
        .await;

        let mut request = json!({"TrailName": "selector-trail"});
        for (member, value) in selectors.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let put = emulator.call("PutEventSelectors", &request).await;
        let put = put.expect_ok(&format!("PutEventSelectors: {case}")).clone();
        assert_eq!(
            put["TrailARN"],
            json!(&arn),
            "case {case}: expected PutEventSelectors to report TrailARN {arn}, got {put:?}"
        );
        assert_eq!(
            selectors_of(&put),
            expected,
            "case {case}: PutEventSelectors did not report the stored selectors, got {put:?}"
        );

        let got = emulator
            .call("GetEventSelectors", &json!({"TrailName": "selector-trail"}))
            .await;
        let got = got.expect_ok(&format!("GetEventSelectors: {case}")).clone();
        assert_eq!(
            (got["TrailARN"].clone(), selectors_of(&got)),
            (json!(&arn), expected),
            "case {case}: GetEventSelectors did not report what was configured, got {got:?}"
        );

        // A trail that carries selectors of its own says so.
        let trail = emulator
            .call("GetTrail", &json!({"Name": "selector-trail"}))
            .await;
        let trail = trail.expect_ok(&format!("GetTrail: {case}"))["Trail"].clone();
        assert_eq!(
            trail["HasCustomEventSelectors"],
            json!(true),
            "case {case}: expected HasCustomEventSelectors true, got {trail:?}"
        );
    }
}

/// The two kinds of selector are mutually exclusive on a trail: configuring one
/// REPLACES the other, and the reply and every later read report only the kind that is
/// now configured.
#[tokio::test]
async fn one_kind_of_selector_replaces_the_other() {
    let emulator = Emulator::new();
    create_trail(
        &emulator,
        HOME_REGION,
        &json!({"Name": "replaced", "S3BucketName": "bucket-a"}),
    )
    .await;

    let basic = json!({
        "TrailName": "replaced",
        "EventSelectors": [{"ReadWriteType": "WriteOnly", "IncludeManagementEvents": true}],
    });
    let advanced = json!({
        "TrailName": "replaced",
        "AdvancedEventSelectors": [{
            "Name": "management events",
            "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
        }],
    });

    // (case, the request to apply, the expected (EventSelectors, AdvancedEventSelectors))
    let steps: Vec<(&str, &Value, (Value, Value))> = vec![
        (
            "basic selectors on a trail nothing configured",
            &basic,
            (
                json!([{
                    "ReadWriteType": "WriteOnly",
                    "IncludeManagementEvents": true,
                    "DataResources": [],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ),
        (
            "advanced selectors over basic ones",
            &advanced,
            (
                Value::Null,
                json!([{
                    "Name": "management events",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                }]),
            ),
        ),
        (
            "basic selectors over advanced ones",
            &basic,
            (
                json!([{
                    "ReadWriteType": "WriteOnly",
                    "IncludeManagementEvents": true,
                    "DataResources": [],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ),
    ];

    for (case, request, expected) in steps {
        let put = emulator.call("PutEventSelectors", request).await;
        assert_eq!(
            selectors_of(put.expect_ok(case)),
            expected,
            "case {case}: PutEventSelectors did not replace the configured selectors, got {put:?}"
        );
        let got = emulator
            .call("GetEventSelectors", &json!({"TrailName": "replaced"}))
            .await;
        assert_eq!(
            selectors_of(got.expect_ok(case)),
            expected,
            "case {case}: GetEventSelectors did not report the replacement, got {got:?}"
        );
    }
}

/// A selector set outside the rules the model documents is refused with
/// `InvalidEventSelectorsException` (HTTP 400) — including a value outside a modelled
/// constraint, which the boundary refuses before the handler runs and which this
/// service answers with that same modelled error rather than a generic
/// `ValidationException` — and the trail keeps the selectors it already carried. The
/// rows differ in WHICH rule is broken: the two kinds together or neither, a count
/// past a published bound, and a value outside a published catalog.
#[tokio::test]
async fn a_selector_set_outside_the_documented_rules_is_refused() {
    let emulator = Emulator::new();
    create_trail(
        &emulator,
        HOME_REGION,
        &json!({"Name": "refusals", "S3BucketName": "bucket-a"}),
    )
    .await;
    // The trail starts out carrying selectors of its own, so a refusal that wrongly
    // wrote would show up as a changed read.
    let configured = json!([{
        "ReadWriteType": "ReadOnly",
        "IncludeManagementEvents": true,
        "DataResources": [],
        "ExcludeManagementEventSources": [],
    }]);
    emulator
        .call(
            "PutEventSelectors",
            &json!({"TrailName": "refusals", "EventSelectors": [{"ReadWriteType": "ReadOnly"}]}),
        )
        .await
        .expect_ok("PutEventSelectors: the configuration the refusals must not disturb");

    // (case, the selectors the request carries, the error code the refusal carries)
    let rows: Vec<(String, Value, &str)> = vec![
        (
            "both kinds of selector at once".to_string(),
            json!({
                "EventSelectors": [{"ReadWriteType": "All"}],
                "AdvancedEventSelectors": [{
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                }],
            }),
            "InvalidEventSelectorsException",
        ),
        (
            "neither kind of selector".to_string(),
            json!({}),
            "InvalidEventSelectorsException",
        ),
        (
            format!(
                "{} event selectors, one past the documented maximum",
                EVENT_SELECTORS_PER_TRAIL_LIMIT + 1
            ),
            json!({
                "EventSelectors": selectors_with_data_resources(EVENT_SELECTORS_PER_TRAIL_LIMIT + 1, 0)
            }),
            "InvalidEventSelectorsException",
        ),
        (
            format!(
                "{} data resources, one past the documented maximum",
                DATA_RESOURCES_PER_TRAIL_LIMIT + 1
            ),
            json!({
                "EventSelectors": selectors_with_data_resources(
                    EVENT_SELECTORS_PER_TRAIL_LIMIT,
                    DATA_RESOURCES_PER_TRAIL_LIMIT + 1,
                )
            }),
            "InvalidEventSelectorsException",
        ),
        (
            format!(
                "{} advanced condition values, one past the documented maximum",
                ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT + 1
            ),
            json!({
                "AdvancedEventSelectors": advanced_selector_with_values(
                    5,
                    ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT + 1,
                )
            }),
            "InvalidEventSelectorsException",
        ),
        (
            // `ReadWriteType` is a modelled enum, so the boundary refuses a value
            // outside `READ_WRITE_TYPE_VALUES` before the handler sees the request —
            // as `InvalidEventSelectorsException`, the error the model's own doc for
            // that shape names this case under ("Specify a valid value for a
            // parameter. For example, specifying the ReadWriteType parameter with a
            // value of read-only is not valid").
            "a ReadWriteType outside the model's enum".to_string(),
            json!({"EventSelectors": [{"ReadWriteType": "WriteMostly"}]}),
            "InvalidEventSelectorsException",
        ),
        (
            // A real CloudTrail resource type, but one only advanced event selectors
            // reach — basic selectors carry `BASIC_DATA_RESOURCE_TYPES` alone.
            "a data-resource type basic selectors cannot carry".to_string(),
            json!({"EventSelectors": [{
                "DataResources": [{"Type": "AWS::S3Express::Object", "Values": ["arn:aws:s3express"]}],
            }]}),
            "InvalidEventSelectorsException",
        ),
        (
            "a management event source that cannot be excluded".to_string(),
            json!({"EventSelectors": [{"ExcludeManagementEventSources": ["s3.amazonaws.com"]}]}),
            "InvalidEventSelectorsException",
        ),
        (
            // A selector that excludes management events and names no data resource
            // selects nothing at all; it is refused rather than installed, and the row
            // below that carries the same `IncludeManagementEvents` WITH a data resource
            // is served. The exact sentence is pinned in
            // `tests/all/selectors_that_log_nothing.rs`.
            "one selector carrying only IncludeManagementEvents false".to_string(),
            json!({"EventSelectors": [{"IncludeManagementEvents": false}]}),
            "InvalidEventSelectorsException",
        ),
        (
            "an advanced field selector stating no condition".to_string(),
            json!({"AdvancedEventSelectors": [{"FieldSelectors": [{"Field": "eventCategory"}]}]}),
            "InvalidEventSelectorsException",
        ),
    ];

    for (case, selectors, code) in rows {
        let mut request = json!({"TrailName": "refusals"});
        for (member, value) in selectors.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        emulator
            .call("PutEventSelectors", &request)
            .await
            .expect_error(&case, 400, code);

        let got = emulator
            .call("GetEventSelectors", &json!({"TrailName": "refusals"}))
            .await;
        assert_eq!(
            selectors_of(got.expect_ok(&case)),
            (configured.clone(), Value::Null),
            "case {case}: a refused request changed the configured selectors, got {got:?}"
        );
    }
}

/// Both operations read `TrailName` as a bare name or a trail ARN: a malformed value is
/// refused as the form it was written in, and a well-formed one naming no trail is
/// `TrailNotFoundException`. The rows differ in the form and in which rule the value
/// breaks.
#[tokio::test]
async fn a_malformed_or_unknown_trail_identifier_is_refused() {
    let emulator = Emulator::new();
    create_trail(
        &emulator,
        HOME_REGION,
        &json!({"Name": "known-trail", "S3BucketName": "bucket-a"}),
    )
    .await;

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
        ("a name that is too short", "ab", 400, "InvalidTrailNameException"),
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
            // CloudTrail answers a not-found trail with 400, not the 404 its model
            // annotates; see `state::TRAIL_NOT_FOUND_STATUS`.
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
            .call("GetEventSelectors", &json!({"TrailName": trail_name}))
            .await
            .expect_error(&format!("GetEventSelectors: {case}"), status, code);
        emulator
            .call(
                "PutEventSelectors",
                &json!({"TrailName": trail_name, "EventSelectors": [{"ReadWriteType": "All"}]}),
            )
            .await
            .expect_error(&format!("PutEventSelectors: {case}"), status, code);
    }
}

/// "The PutEventSelectors operation must be called from the Region in which the trail
/// was created; otherwise, an InvalidHomeRegionException exception is thrown." The read
/// carries no such rule, so the same identifiers answer the configured selectors.
#[tokio::test]
async fn put_event_selectors_runs_only_in_the_trail_home_region() {
    let emulator = Emulator::new();
    let arn = create_trail(
        &emulator,
        HOME_REGION,
        &json!({"Name": "home-bound", "S3BucketName": "bucket-a", "IsMultiRegionTrail": true}),
    )
    .await;

    // (case, TrailName as written, the Region the call is made from)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a bare name from another Region",
            "home-bound".to_string(),
            OTHER_REGION,
        ),
        (
            "the home-Region ARN from another Region",
            arn.clone(),
            OTHER_REGION,
        ),
    ];

    for (case, trail_name, region) in rows {
        emulator
            .call_in(
                "PutEventSelectors",
                region,
                &json!({"TrailName": trail_name, "EventSelectors": [{"ReadWriteType": "All"}]}),
            )
            .await
            .expect_error(
                &format!("PutEventSelectors: {case}"),
                400,
                "InvalidHomeRegionException",
            );
    }

    // The trail was never configured, so the read still reports the default — from
    // either Region, by ARN.
    for region in [HOME_REGION, OTHER_REGION] {
        let got = emulator
            .call_in("GetEventSelectors", region, &json!({"TrailName": arn}))
            .await;
        assert_eq!(
            selectors_of(got.expect_ok(region)),
            (default_selectors(), Value::Null),
            "case GetEventSelectors from {region}: expected the default selector, got {got:?}"
        );
    }
}

/// One row of [`configuring_one_trail_leaves_every_other_trail_alone`]: the case name,
/// the trail to configure, how the request addresses it, the selectors the request
/// carries, and the `(EventSelectors, AdvancedEventSelectors)` every later read of that
/// trail must report.
type IsolationRow = (&'static str, &'static str, String, Value, (Value, Value));

/// Event selectors belong to the ONE trail the request's identifier resolves to: a
/// `PutEventSelectors` call writes that trail and no other, and applying the same
/// request again reports the same configuration — the operation is idempotent, because
/// it REPLACES what the trail carried rather than accumulating onto it.
///
/// Every row is checked against every trail in the registry, so a handler that wrote a
/// registry-wide field instead of the resolved record, or that appended to the stored
/// set, shows up as a changed read on a trail the row never named. The rows differ in
/// the identifier FORM the request addresses the trail by (a bare name, a trail ARN), in
/// the Region the trail is homed in, and in the KIND of selector configured.
#[tokio::test]
async fn configuring_one_trail_leaves_every_other_trail_alone() {
    let emulator = Emulator::new();

    // (trail name, the Region the trail is homed in): a plain name, one carrying the
    // punctuation a trail name may hold, and one homed outside this emulator's default
    // Region.
    let trails = [
        ("alpha-trail", HOME_REGION),
        ("beta.trail", HOME_REGION),
        ("west-trail", "us-west-2"),
    ];
    // trail name -> (its home Region, the ARN the service minted for it)
    let mut homes: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    for (index, (name, region)) in trails.into_iter().enumerate() {
        let arn = create_trail(
            &emulator,
            region,
            &json!({"Name": name, "S3BucketName": format!("bucket-{index}")}),
        )
        .await;
        homes.insert(name, (region, arn));
    }

    // (case, the trail to configure, how the request addresses it, the selectors it
    //  carries, the expected (EventSelectors, AdvancedEventSelectors) of every read of
    //  that trail afterwards)
    let rows: Vec<IsolationRow> = vec![
        (
            "a bare name, basic management-event selectors",
            "alpha-trail",
            "alpha-trail".to_string(),
            json!({"EventSelectors": [{"ReadWriteType": "ReadOnly"}]}),
            (
                json!([{
                    "ReadWriteType": "ReadOnly",
                    "IncludeManagementEvents": true,
                    "DataResources": [],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ),
        (
            "the trail ARN, advanced selectors",
            "beta.trail",
            homes["beta.trail"].1.clone(),
            json!({"AdvancedEventSelectors": [{
                "Name": "management events",
                "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
            }]}),
            (
                Value::Null,
                json!([{
                    "Name": "management events",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                }]),
            ),
        ),
        (
            "a trail homed in another Region, by bare name there, data-event selectors",
            "west-trail",
            "west-trail".to_string(),
            json!({"EventSelectors": [{
                "ReadWriteType": "WriteOnly",
                "IncludeManagementEvents": false,
                "DataResources": [{
                    "Type": BASIC_DATA_RESOURCE_TYPES[0],
                    "Values": ["arn:aws:dynamodb:us-west-2:000000000000:table/orders"],
                }],
            }]}),
            (
                json!([{
                    "ReadWriteType": "WriteOnly",
                    "IncludeManagementEvents": false,
                    "DataResources": [{
                        "Type": BASIC_DATA_RESOURCE_TYPES[0],
                        "Values": ["arn:aws:dynamodb:us-west-2:000000000000:table/orders"],
                    }],
                    "ExcludeManagementEventSources": [],
                }]),
                Value::Null,
            ),
        ),
    ];

    // What each trail must report. Every trail starts out at the documented default,
    // and a row moves exactly one entry.
    let mut expected: BTreeMap<&str, (Value, Value)> = trails
        .into_iter()
        .map(|(name, _)| (name, (default_selectors(), Value::Null)))
        .collect();

    for (case, trail, identifier, selectors, configured) in rows {
        let (region, arn) = homes[trail].clone();
        let mut request = json!({"TrailName": identifier});
        for (member, value) in selectors.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }

        // Applied TWICE: the second application is the idempotence case, and must
        // report exactly what the first did.
        for attempt in 1..=2 {
            let put = emulator
                .call_in("PutEventSelectors", region, &request)
                .await;
            let put = put
                .expect_ok(&format!("PutEventSelectors attempt {attempt}: {case}"))
                .clone();
            assert_eq!(
                (put["TrailARN"].clone(), selectors_of(&put)),
                (json!(&arn), configured.clone()),
                "case {case} (attempt {attempt}): expected PutEventSelectors to report \
                 TrailARN {arn} carrying the selectors it was given, got {put:?}"
            );
        }
        expected.insert(trail, configured);

        // Every trail — the one just configured and the ones the row never named —
        // reports its own configuration, read by ARN so no name resolution is in play.
        for (name, _) in trails {
            let (_, other_arn) = &homes[name];
            let got = emulator
                .call("GetEventSelectors", &json!({"TrailName": other_arn}))
                .await;
            let got = got
                .expect_ok(&format!("GetEventSelectors {name}: {case}"))
                .clone();
            assert_eq!(
                (got["TrailARN"].clone(), selectors_of(&got)),
                (json!(other_arn), expected[name].clone()),
                "case {case}: expected trail {name} to report its own selectors, got {got:?}"
            );
        }
    }
}
