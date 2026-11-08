// SPDX-License-Identifier: CC0-1.0

//! How CloudTrail refuses a VALUE outside the set its parameter admits, over every such
//! parameter of an event selector.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production router
//! serves — so what is asserted is what a client sees: the HTTP status, the error code and
//! the message.
//!
//! The rule under test: a value outside the set an event-selector parameter admits is
//! refused with `InvalidEventSelectorsException` (HTTP 400) and the fixed sentence
//! `Value <value> for <Member> is invalid.` — the value exactly as the request sent it,
//! bare, and the parameter named by its API path inside the selector it sits in. It holds
//! whichever side settles the set: the modelled enum the boundary walk checks
//! (`ReadWriteType`) and the catalogues the handler checks itself (`DataResources.Type`,
//! `ExcludeManagementEventSources`), and it holds wherever in a selector list or a data
//! resource list the offending entry sits — the parameter's name carries no position.
//! A value INSIDE the set is served.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::READ_WRITE_TYPE_VALUES;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    BASIC_DATA_RESOURCE_TYPES, DEFAULT_INCLUDE_MANAGEMENT_EVENTS, DEFAULT_READ_WRITE_TYPE,
    EXCLUDABLE_MANAGEMENT_EVENT_SOURCES,
};

use crate::emulator::Emulator;

/// The status CloudTrail answers its client errors with.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error the model declares for a selector a trail cannot be configured with
/// (`smithy_query(query="operation", operation="PutEventSelectors")`).
const INVALID_EVENT_SELECTORS: &str = "InvalidEventSelectorsException";

/// The sentence under test, as CloudTrail words it.
fn expected_message(member: &str, value: &str) -> String {
    format!("Value {value} for {member} is invalid.")
}

/// A data resource of `resource_type`, with one value its type would address.
fn data_resource(resource_type: &str) -> Value {
    json!({"Type": resource_type, "Values": ["arn:aws:s3:::audit-subject/"]})
}

/// RULE: a value outside the set its event-selector parameter admits is refused with the
/// fixed `Value <value> for <Member> is invalid.` sentence, naming that parameter and
/// echoing the value as sent.
#[tokio::test]
async fn a_value_outside_its_parameters_set_is_refused_by_name_and_value() {
    let emulator = Emulator::new();
    let trail = "value-refusals";
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": "value-refusals-logs"}),
        )
        .await
        .expect_ok("CreateTrail");

    // (case, the EventSelectors the request carries, the parameter named, the value echoed)
    let mut rows: Vec<(String, Value, &str, String)> = Vec::new();

    // `DataResource$Type`: the model's doc for that member names three basic types, so
    // every other resource type — a real AWS type reached only through advanced selectors,
    // a type of another service, a mis-cased spelling of an admitted one, a value that is
    // not a type at all and an empty one — is outside the set.
    for resource_type in [
        "AWS::S3Express::Object",
        "AWS::DynamoDB::Stream",
        "AWS::EC2::Instance",
        "aws::s3::object",
        "S3::Object",
        "",
    ] {
        rows.push((
            format!("a DataResources.Type of {resource_type:?}"),
            json!([{"DataResources": [data_resource(resource_type)]}]),
            "DataResources.Type",
            resource_type.to_string(),
        ));
    }
    // The parameter is named the same whichever selector and whichever data resource of
    // that selector carried the value: no position is part of its name.
    rows.push((
        "a bad DataResources.Type in the second data resource of the second selector"
            .to_string(),
        json!([
            {"DataResources": [data_resource(BASIC_DATA_RESOURCE_TYPES[0])]},
            {"DataResources": [
                data_resource(BASIC_DATA_RESOURCE_TYPES[1]),
                data_resource("AWS::SQS::Queue"),
            ]},
        ]),
        "DataResources.Type",
        "AWS::SQS::Queue".to_string(),
    ));

    // `EventSelector$ReadWriteType`: a modelled enum, so the boundary walk settles the set
    // — the refusal is the same sentence whichever side of the handler decides it.
    for read_write_type in ["Sideways", "read-only", "all", "WriteOnlyish"] {
        rows.push((
            format!("a ReadWriteType of {read_write_type:?}"),
            json!([{"ReadWriteType": read_write_type, "IncludeManagementEvents": true}]),
            "ReadWriteType",
            read_write_type.to_string(),
        ));
    }
    rows.push((
        "a bad ReadWriteType on the second selector".to_string(),
        json!([
            {"ReadWriteType": READ_WRITE_TYPE_VALUES[0], "IncludeManagementEvents": true},
            {"ReadWriteType": "Everything", "IncludeManagementEvents": true},
        ]),
        "ReadWriteType",
        "Everything".to_string(),
    ));

    // `EventSelector$ExcludeManagementEventSources`: the model's doc for that member names
    // the two sources that can be filtered out, so every other source is outside the set.
    for source in ["s3.amazonaws.com", "KMS.amazonaws.com", "kms", ""] {
        rows.push((
            format!("an excluded management event source of {source:?}"),
            json!([{
                "IncludeManagementEvents": true,
                "ExcludeManagementEventSources": [source],
            }]),
            "ExcludeManagementEventSources",
            source.to_string(),
        ));
    }

    for (case, selectors, member, value) in rows {
        let reply = emulator
            .call(
                "PutEventSelectors",
                &json!({"TrailName": trail, "EventSelectors": selectors}),
            )
            .await;
        reply.expect_error(&case, CLIENT_ERROR_STATUS, INVALID_EVENT_SELECTORS);
        let expected = expected_message(member, &value);
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }
}

/// RULE's other side: a value INSIDE the set its parameter admits is served, so the refusal
/// above is about the value and not about the parameter being present.
///
/// Every entry of each catalogue is driven, including the ones the refusals above did not
/// cite: the three basic data-resource types (the model's doc for `DataResource$Type`), the
/// two excludable management event sources (the doc for
/// `EventSelector$ExcludeManagementEventSources`) and every `ReadWriteType` the model
/// declares.
#[tokio::test]
async fn every_value_the_catalogues_admit_is_served() {
    let emulator = Emulator::new();
    let trail = "value-catalogues";
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": "value-catalogues-logs"}),
        )
        .await
        .expect_ok("CreateTrail");

    let mut rows: Vec<(String, Value)> = Vec::new();
    for resource_type in BASIC_DATA_RESOURCE_TYPES {
        rows.push((
            format!("the basic data-resource type {resource_type}"),
            json!([{"DataResources": [data_resource(resource_type)]}]),
        ));
    }
    for source in EXCLUDABLE_MANAGEMENT_EVENT_SOURCES {
        rows.push((
            format!("the excludable management event source {source}"),
            json!([{"IncludeManagementEvents": true, "ExcludeManagementEventSources": [source]}]),
        ));
    }
    for read_write_type in READ_WRITE_TYPE_VALUES {
        rows.push((
            format!("the ReadWriteType {read_write_type}"),
            json!([{"ReadWriteType": read_write_type, "IncludeManagementEvents": true}]),
        ));
    }
    // Every catalogue at once, in one selector set the service has to accept whole.
    rows.push((
        "one selector carrying a value from each catalogue".to_string(),
        json!([{
            "ReadWriteType": READ_WRITE_TYPE_VALUES[0],
            "IncludeManagementEvents": true,
            "DataResources": BASIC_DATA_RESOURCE_TYPES
                .iter()
                .map(|resource_type| data_resource(resource_type))
                .collect::<Vec<Value>>(),
            "ExcludeManagementEventSources": [EXCLUDABLE_MANAGEMENT_EVENT_SOURCES[0]],
        }]),
    ));

    for (case, selectors) in rows {
        let reply = emulator
            .call(
                "PutEventSelectors",
                &json!({"TrailName": trail, "EventSelectors": selectors.clone()}),
            )
            .await;
        let body = reply.expect_ok(&case);
        // The set the service has to have stored: what the request sent, with each member
        // the request left out carrying its documented default.
        let expected = stored_form(&selectors);
        let reported = body.get("EventSelectors").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the write to report {expected}, got {reported}"
        );
        // And what was stored is what a later read reports, so the accepted set is read
        // back through the reader rather than only off the write's own answer.
        let read = emulator
            .call("GetEventSelectors", &json!({"TrailName": trail}))
            .await;
        let read = read
            .expect_ok(&format!("GetEventSelectors after {case}"))
            .get("EventSelectors")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            read, expected,
            "case {case:?}: expected a read to report the stored set {expected}, got {read}"
        );
    }
}

/// The selector set the service stores for `selectors`: every member the request sent, with
/// each one it left out carrying the default the model documents for it
/// (`EventSelector$ReadWriteType` "By default, the value is All",
/// `EventSelector$IncludeManagementEvents` "By default, the value is true", and the two
/// lists empty).
fn stored_form(selectors: &Value) -> Value {
    let sent = selectors.as_array().expect("the row carries a selector list");
    Value::Array(
        sent.iter()
            .map(|selector| {
                let member = |name: &str| selector.get(name).cloned();
                json!({
                    "ReadWriteType": member("ReadWriteType")
                        .unwrap_or_else(|| json!(DEFAULT_READ_WRITE_TYPE)),
                    "IncludeManagementEvents": member("IncludeManagementEvents")
                        .unwrap_or_else(|| json!(DEFAULT_INCLUDE_MANAGEMENT_EVENTS)),
                    "DataResources": member("DataResources").unwrap_or_else(|| json!([])),
                    "ExcludeManagementEventSources": member("ExcludeManagementEventSources")
                        .unwrap_or_else(|| json!([])),
                })
            })
            .collect(),
    )
}
