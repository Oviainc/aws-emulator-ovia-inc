// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusal a request BODY that omits a property the
//! model marks `@required` earns, over every operation of this service that reads one
//! from the body.
//!
//! The rule under test: the schema of the request body is judged by the REST front end
//! in front of this service, as the body is read and before any value is validated and
//! before any handler runs, so a body that does not carry a required property is HTTP
//! 400 with that layer's error code (`BadRequestException`, which no operation of this
//! service declares) and its schema finding — `Invalid request body: [object has
//! missing required properties ([<the omitted properties>])]` — each property named as
//! the MODEL declares it (Pascal case, which is also how a caller must send it) and
//! taken from the request rather than from any one recorded call. Nothing is stored,
//! read or changed for such a request.
//!
//! Rows differ along what the rule generalises over: the operation (every route that
//! takes a required body property, including the REST-bound `PUT /tags/{ResourceArn}`),
//! WHICH property is omitted (so the finding's variable part has to come from the
//! request), the TYPE of the omitted member (a string, a list, a map), whether the
//! omitted property sits at the top of the body or NESTED inside one of its
//! structures, and whether the body carries the operation's OTHER members or none of
//! them. The boundary of the rule is asserted too: a property that is PRESENT but
//! outside a modelled bound keeps the service's own modelled parameter error, and
//! `UpdateLink`'s authorization decision still comes before the absence it reports.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization including the `@httpLabel` ARN of the tagging route, the
//! modelled-constraint walk and the handlers), so what is asserted is what a signed AWS
//! SDK request observes.

use serde_json::{json, Value};

use super::front_end_refusals::refusal_message;
use super::list_tags_for_resource::{list_tags, percent_encode_segment, tag_map, tags_json};
use super::sinks_and_links::{
    attached_link_arns, call, call_as, create_link_ok, create_sink_ok, dispatch_parsed, emulator,
    error_code, field, AKID, RESOURCE_TYPES,
};
use super::untag_resource::rest_request;

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// A second access key id, so a case can ask about a link another source account owns.
const OTHER_AKID: &str = "AKIDOAMMISSING2";

/// The whole message the front end states for a body missing `properties`.
///
/// Written out here rather than built from the crate under test: the envelope
/// (`Invalid request body: [`, `]`) and the finding (`object has missing required
/// properties ([`, `])`, with each property double-quoted and separated by a bare
/// comma) are fixed text, and the property names are the only variable part — which is
/// what each row varies.
fn expected_message(properties: &[&str]) -> String {
    let quoted: Vec<String> = properties
        .iter()
        .map(|property| format!("\"{property}\""))
        .collect();
    format!(
        "Invalid request body: [object has missing required properties ([{}])]",
        quoted.join(",")
    )
}

/// Assert `uri` refuses `body` as a body missing `property`, and answer nothing.
async fn assert_missing_property(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    uri: &str,
    body: &Value,
    property: &str,
) {
    let (status, refused) = call(router, uri, body, REGION).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "BadRequestException"),
        "case {case:?}: expected 400 BadRequestException, got {status} {refused:?}"
    );
    assert_eq!(
        refusal_message(&refused),
        expected_message(&[property]),
        "case {case:?}: expected the front end's missing-property finding, got {refused:?}"
    );
}

#[tokio::test]
async fn a_body_missing_a_required_property_is_the_front_ends_refusal() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "missing-prop-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let seeded = tag_map(&[("env", "dev")]);

    // One row per (operation, omitted property): every operation of this service that
    // reads a `@required` property from the request body
    // (`smithy_query(query="digest")`), with the property it omits.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "CreateSink without Name",
            "/CreateSink",
            json!({}),
            "Name",
        ),
        (
            "CreateSink with tags but no Name",
            "/CreateSink",
            json!({ "Tags": tags_json(&seeded) }),
            "Name",
        ),
        (
            "CreateLink without LabelTemplate",
            "/CreateLink",
            json!({
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "LabelTemplate",
        ),
        (
            "CreateLink without ResourceTypes (a list member)",
            "/CreateLink",
            json!({
                "LabelTemplate": "$AccountName",
                "SinkIdentifier": sink_arn.clone(),
            }),
            "ResourceTypes",
        ),
        (
            "CreateLink without SinkIdentifier",
            "/CreateLink",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [RESOURCE_TYPES[0]],
            }),
            "SinkIdentifier",
        ),
        (
            "GetSink without Identifier",
            "/GetSink",
            json!({ "IncludeTags": true }),
            "Identifier",
        ),
        (
            "GetLink without Identifier",
            "/GetLink",
            json!({}),
            "Identifier",
        ),
        (
            "DeleteSink without Identifier",
            "/DeleteSink",
            json!({}),
            "Identifier",
        ),
        (
            "DeleteLink without Identifier",
            "/DeleteLink",
            json!({}),
            "Identifier",
        ),
        (
            "GetSinkPolicy without SinkIdentifier",
            "/GetSinkPolicy",
            json!({}),
            "SinkIdentifier",
        ),
        (
            "PutSinkPolicy without Policy",
            "/PutSinkPolicy",
            json!({ "SinkIdentifier": sink_arn.clone() }),
            "Policy",
        ),
        (
            "PutSinkPolicy without SinkIdentifier",
            "/PutSinkPolicy",
            json!({ "Policy": "{}" }),
            "SinkIdentifier",
        ),
        (
            "ListAttachedLinks without SinkIdentifier",
            "/ListAttachedLinks",
            json!({ "MaxResults": 1 }),
            "SinkIdentifier",
        ),
        (
            "UpdateLink without Identifier",
            "/UpdateLink",
            json!({ "ResourceTypes": [RESOURCE_TYPES[0]] }),
            "Identifier",
        ),
        (
            // A NESTED required property is named by its own property name, not by the
            // path of the objects it sits in: `LogGroupConfiguration$Filter` is
            // `@required` inside `LinkConfiguration`
            // (`smithy_query(query="shape",shape="LogGroupConfiguration")`).
            "CreateLink whose LogGroupConfiguration omits Filter",
            "/CreateLink",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "SinkIdentifier": sink_arn.clone(),
                "LinkConfiguration": { "LogGroupConfiguration": {} },
            }),
            "Filter",
        ),
        (
            "UpdateLink whose MetricConfiguration omits Filter",
            "/UpdateLink",
            json!({
                "Identifier": link_arn.clone(),
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "LinkConfiguration": { "MetricConfiguration": {} },
            }),
            "Filter",
        ),
        (
            // `UpdateLink` takes its authorization decision on the link `Identifier`
            // names BEFORE it judges anything else the request carries, so this
            // absence is reported by the handler rather than by the pre-handler walk
            // — in the SAME words, because it is the same rule about the same body.
            "UpdateLink on a link the caller owns, without ResourceTypes",
            "/UpdateLink",
            json!({ "Identifier": link_arn.clone() }),
            "ResourceTypes",
        ),
    ];

    for (case, uri, body, property) in &rows {
        assert_missing_property(&router, case, uri, body, property).await;
    }

    // The REST-bound tagging route reads `Tags` (a MAP) from the body while its
    // `ResourceArn` travels as an `@httpLabel`, so a body carrying no `Tags` is the
    // same front-end refusal — and the ARN it names is left alone.
    let (status, refused) = dispatch_parsed(
        &router,
        rest_request(
            "PUT",
            &format!("/tags/{}", percent_encode_segment(&sink_arn)),
            &json!({}),
            REGION,
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "BadRequestException"),
        "case \"TagResource without Tags\": expected 400 BadRequestException, got {status} \
         {refused:?}"
    );
    assert_eq!(
        refusal_message(&refused),
        expected_message(&["Tags"]),
        "case \"TagResource without Tags\": expected the missing-property finding, got {refused:?}"
    );

    // A body carrying NONE of an operation's required properties is refused for one of
    // them — absence is found while reading the request, which stops at the first
    // property it misses — and the finding still names a property that request really
    // omitted.
    let (status, refused) = call(&router, "/CreateLink", &json!({}), REGION).await;
    let message = refusal_message(&refused);
    let admissible: Vec<String> = ["LabelTemplate", "ResourceTypes", "SinkIdentifier"]
        .iter()
        .map(|property| expected_message(&[property]))
        .collect();
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "BadRequestException"),
        "case \"CreateLink with an empty body\": expected 400 BadRequestException, got {status} \
         {refused:?}"
    );
    assert!(
        admissible.contains(&message),
        "case \"CreateLink with an empty body\": expected a finding naming one of the three \
         required properties, got {message:?}"
    );

    // The finding states a LIST of properties, so its plural form is part of the rule
    // even though the read above cannot surface two absences at once: several names go
    // inside ONE pair of brackets, comma-separated, in the order they are reported.
    // Asserted through the refusal both the pre-handler seam and a handler build, since
    // no request can drive it over the wire.
    for properties in [
        vec!["LabelTemplate".to_string(), "SinkIdentifier".to_string()],
        vec![
            "LabelTemplate".to_string(),
            "ResourceTypes".to_string(),
            "SinkIdentifier".to_string(),
        ],
    ] {
        let named: Vec<&str> = properties.iter().map(String::as_str).collect();
        assert_eq!(
            service_com_amazonaws_oam_oamservice::state::missing_properties_rejection(&properties)
                .message,
            expected_message(&named),
            "case {named:?}: expected one finding listing every absent property"
        );
    }

    // Not one of the refusals read or wrote anything: the one sink still answers, it
    // still carries exactly the one link, that link still shares what it was created
    // with, and it holds no tags the refused `TagResource` could have written.
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": sink_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, field("after the refusals", &got, "Arn")),
        (200, sink_arn.clone()),
        "the stored sink must still answer, got {status} {got:?}"
    );
    assert_eq!(
        attached_link_arns(&router, "after the refusals", &sink_arn, None, REGION).await,
        vec![link_arn.clone()],
        "the refusals must not have stored or removed a link"
    );
    let (status, link) = call(
        &router,
        "/GetLink",
        &json!({ "Identifier": link_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored link must still answer, got {status} {link:?}"
    );
    assert_eq!(
        link.get("ResourceTypes").cloned(),
        Some(json!([RESOURCE_TYPES[0]])),
        "the refused UpdateLink must have changed nothing, got {link:?}"
    );
    let (status, tags) = list_tags(&router, &sink_arn, REGION).await;
    assert_eq!(
        (status, tags.get("Tags").cloned()),
        (200, Some(json!({}))),
        "the refused TagResource must have written nothing, got {status} {tags:?}"
    );
}

#[tokio::test]
async fn the_absence_refusal_is_bounded_by_the_rules_either_side_of_it() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "boundary-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let other_link_arn = create_link_ok(&router, &sink_arn, OTHER_AKID, REGION).await;

    // A property that is PRESENT but outside a modelled bound is not an absent
    // property: it is the service's own modelled parameter error, so one request can
    // never earn the front end's code for a rule the service reports.
    let present_but_invalid: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an empty LabelTemplate, one character below the modelled minimum",
            "/CreateLink",
            json!({
                "LabelTemplate": "",
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "InvalidParameterException",
        ),
        (
            "an empty ResourceTypes, one short of the modelled minimum",
            "/CreateLink",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "InvalidParameterException",
        ),
        (
            "a ResourceTypes naming one type twice",
            "/UpdateLink",
            json!({
                "Identifier": link_arn.clone(),
                "ResourceTypes": [RESOURCE_TYPES[0], RESOURCE_TYPES[0]],
            }),
            "InvalidParameterException",
        ),
        (
            "a MaxResults below the modelled range",
            "/ListAttachedLinks",
            json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": 0 }),
            "InvalidParameterException",
        ),
    ];
    for (case, uri, body, expected_code) in &present_but_invalid {
        let (status, refused) = call(&router, uri, body, REGION).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} {refused:?}"
        );
    }

    // And the absence `UpdateLink` reports is reported only once the caller is allowed
    // to act on the link at all: the identical body naming a link this caller does not
    // own is the authorization refusal, which discloses nothing about the request's
    // other members.
    let (status, refused) = call_as(
        &router,
        "/UpdateLink",
        &json!({ "Identifier": other_link_arn.clone() }),
        AKID,
        REGION,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "a link of another source account: expected 403 AccessDeniedException, got {status} \
         {refused:?}"
    );
}
