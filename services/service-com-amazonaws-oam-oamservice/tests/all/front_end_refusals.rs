// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusals the layers IN FRONT of this service answer,
//! and for the tag projection `IncludeTags` controls.
//!
//! Four rules are under test, each asserted over a table of inputs rather than one
//! recorded case:
//!
//! 1. Membership of a modelled enum is a rule of the request BODY: an element outside
//!    the `ResourceType` enum is refused by the layer that validates the body — HTTP
//!    400 `BadRequestException`, with the clause naming the value sent and the values
//!    the enum declares — wherever that enum is bound (`CreateLink` and `UpdateLink`),
//!    and nothing is stored for the refused request.
//! 2. `IncludeTags` SUPPRESSES the tag projection rather than requesting it: a read
//!    that leaves the member out reports the resource's stored tag set, the same set
//!    `ListTagsForResource` answers for that ARN, and only an explicit `false` leaves
//!    `Tags` out — one rule for the one modelled `IncludeTags` shape, which `GetSink`,
//!    `GetLink` and `UpdateLink` all bind.
//! 3. The TAGGING path answers its own refusals, distinct from the control plane's: an
//!    ARN whose syntax is not one of this service's is HTTP 400 `BadRequestException`
//!    with one fixed sentence, and an ARN naming no stored resource is HTTP 404
//!    `NotFoundException` with another — while the control-plane operations answer
//!    their modelled `ResourceNotFoundException` for the very same resource.
//! 4. `ListAttachedLinks` is authorized against the sink the request names BEFORE the
//!    identifier is judged, so a `SinkIdentifier` that could name no resource at all is
//!    refused by that decision (HTTP 403 `AccessDeniedException`, reported on the sink
//!    kind of the request's Region) rather than by input validation — while a
//!    well-formed identifier naming no sink stays the modelled 404 and a stored sink
//!    with no links stays an empty page.
//! 5. A tag key in the namespace Amazon Web Services reserves for its own tags is not
//!    one a customer may set: the refusal is HTTP 400 `BadRequestException` with one
//!    fixed sentence, the prefix is matched case-insensitively, and nothing is written
//!    — wherever a request sets tags (`TagResource`, `CreateSink`, `CreateLink`).
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization including the `@httpLabel` ARN of the tagging routes, the
//! modelled-constraint walk and the handlers), so what is asserted is what a signed AWS
//! SDK request observes.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_oam_oamservice::state::{
    caller_principal, INVALID_TAGGING_ARN_MESSAGE, RESERVED_TAG_KEY_SET_MESSAGE,
    RESOURCE_NOT_FOUND_MESSAGE, TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::list_tags_for_resource::{list_tags, tag_map, tags_json};
use super::sinks_and_links::{
    call, call_as, create_link_ok, create_sink_ok, emulator, error_code, field, ACCOUNT, AKID,
    MONITORING_AKID, RESOURCE_TYPES,
};
use super::untag_resource::tag;

/// The region every case signs for unless it is asking about another endpoint.
const REGION: &str = "us-west-2";

/// A second region, so a case can ask about a resource that exists at another endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// A second access key id: a link belongs to the source account that created it, so a
/// case needing two links on one sink signs the second with this.
const OTHER_AKID: &str = "AKIDOAMFRONT2";

/// The `message` member of a refusal, whichever casing the answering shape models it
/// with: the modelled error shapes of this service spell it `Message`, the front end's
/// own envelope `message`.
pub(crate) fn refusal_message(body: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    panic!("expected a message member, got {body:?}")
}

/// The body-validation refusal an element outside the `ResourceType` enum earns.
///
/// The clause states the value the request sent and the values the enum declares, in
/// the order the model declares them — [`RESOURCE_TYPES`], which is
/// `smithy_query(query="shape",shape="ResourceType")`'s own value list.
fn expected_enum_refusal(value: &str) -> String {
    let values: Vec<String> = RESOURCE_TYPES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect();
    format!(
        "Invalid request body: [instance value (\"{value}\") not found in enum (possible \
         values: [{}])]",
        values.join(",")
    )
}

/// A `CreateLink` body naming `resource_types` on `sink_identifier`.
fn create_link_body(sink_identifier: &str, resource_types: &[&str]) -> Value {
    json!({
        "LabelTemplate": "$AccountName",
        "ResourceTypes": resource_types,
        "SinkIdentifier": sink_identifier,
    })
}

/// An `UpdateLink` body naming `resource_types` on `identifier`.
fn update_link_body(identifier: &str, resource_types: &[&str]) -> Value {
    json!({ "Identifier": identifier, "ResourceTypes": resource_types })
}

/// Every `LinkArn` `ListAttachedLinks` reports for `sink_identifier`, which must
/// answer.
async fn attached(router: &MultiServiceRouter, case: &str, sink_identifier: &str) -> Vec<String> {
    let (status, listed) = call(
        router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_identifier }),
        REGION,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAttachedLinks expected 200, got {status} {listed:?}"
    );
    listed
        .get("Items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: Items is @required, got {listed:?}"))
        .iter()
        .map(|item| field(case, item, "LinkArn"))
        .collect()
}

#[tokio::test]
async fn an_element_outside_the_modelled_enum_is_the_front_ends_refusal() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "enum-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;

    // Rows differ along HOW the value misses the enum: a type of another service, a
    // modelled value in the wrong case, a modelled value with trailing text, a prefix
    // of one, a value naming a resource kind this service does share but spelled with
    // one segment, and the empty string.
    let rows: &[(&str, &str)] = &[
        ("a type no service declares", "AWS::Bogus::NotARealType"),
        ("a neighbouring Logs type", "AWS::Logs::LogStream"),
        ("a modelled value lower-cased", "aws::logs::loggroup"),
        ("a modelled value with trailing text", "AWS::XRay::Traces"),
        ("a prefix of a modelled value", "AWS::Logs::"),
        ("one segment only", "LogGroup"),
        ("the empty string", ""),
    ];

    for (case, value) in rows {
        // The same value in each of the two operations that bind the enum, and in a
        // list that also carries modelled values — the position in the list cannot
        // change the refusal.
        let bodies: Vec<(String, &str, Value)> = vec![
            (
                format!("CreateLink with {case}"),
                "/CreateLink",
                create_link_body(&sink_arn, &[value]),
            ),
            (
                format!("CreateLink with {case} after a modelled type"),
                "/CreateLink",
                create_link_body(&sink_arn, &[RESOURCE_TYPES[0], value]),
            ),
            (
                format!("UpdateLink with {case}"),
                "/UpdateLink",
                update_link_body(&link_arn, &[value]),
            ),
            (
                format!("UpdateLink with {case} after a modelled type"),
                "/UpdateLink",
                update_link_body(&link_arn, &[RESOURCE_TYPES[1], value]),
            ),
        ];
        for (case, uri, body) in bodies {
            let (status, refused) = call(&router, uri, &body, REGION).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {refused:?}"
            );
            assert_eq!(
                refusal_message(&refused),
                expected_enum_refusal(value),
                "case {case:?}: expected the body validator's enum clause, got {refused:?}"
            );
        }
    }

    // Not one of the refusals stored anything: the sink still carries the one link it
    // had, and that link still shares what it was created with.
    assert_eq!(
        attached(&router, "after the enum refusals", &sink_arn).await,
        vec![link_arn.clone()],
        "a refused request must neither create nor change a link"
    );
    let (status, got) = call(
        &router,
        "/GetLink",
        &json!({ "Identifier": link_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, got.get("ResourceTypes")),
        (200, Some(&json!(["AWS::CloudWatch::Metric"]))),
        "a refused update must leave the link's resource types alone, got {status} {got:?}"
    );

    // The control: EVERY value the modelled catalog declares is accepted by the same
    // operation — including the ones the issue did not cite.
    for resource_type in RESOURCE_TYPES {
        let (status, updated) = call(
            &router,
            "/UpdateLink",
            &update_link_body(&link_arn, &[resource_type]),
            REGION,
        )
        .await;
        assert_eq!(
            (status, updated.get("ResourceTypes")),
            (200, Some(&json!([resource_type]))),
            "case {resource_type:?}: a modelled type must be accepted, got {status} {updated:?}"
        );
    }
}

#[tokio::test]
async fn include_tags_suppresses_the_tag_projection_rather_than_requesting_it() {
    // Rows differ along the tag set the resource carries: none, one, several, and a
    // value the model admits as empty (`TagValue` `@length(0..256)`).
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("no tags at all", BTreeMap::new()),
        ("one tag", tag_map(&[("origin", "create")])),
        (
            "several tags",
            tag_map(&[("env", "prod"), ("owner", "sre"), ("team", "observability")]),
        ),
        ("an empty value", tag_map(&[("empty", "")])),
    ];

    for (row, tags) in rows {
        let router = emulator();
        let mut sink_body = json!({ "Name": "tagged-sink" });
        if !tags.is_empty() {
            sink_body["Tags"] = tags_json(&tags);
        }
        let (status, sink) = call_as(&router, "/CreateSink", &sink_body, MONITORING_AKID, REGION)
            .await;
        assert_eq!(
            status, 200,
            "case {row:?}: CreateSink expected 200, got {status} {sink:?}"
        );
        let sink_arn = field(row, &sink, "Arn");

        let mut link_body = create_link_body(&sink_arn, &[RESOURCE_TYPES[0]]);
        if !tags.is_empty() {
            link_body["Tags"] = tags_json(&tags);
        }
        let (status, link) = call(&router, "/CreateLink", &link_body, REGION).await;
        assert_eq!(
            status, 200,
            "case {row:?}: CreateLink expected 200, got {status} {link:?}"
        );
        let link_arn = field(row, &link, "Arn");

        // An empty tag set is REPORTED as an empty map by every read that reports the
        // set at all: that is the answer to "what tags does this resource have" for a
        // resource carrying none, and it is what the tag list answers for the same ARN.
        let stored = tags_json(&tags);

        // Three ways to ask, over every operation that binds the member: omitted and
        // `true` report the stored set, `false` omits it.
        let asks: &[(&str, Option<bool>, bool)] = &[
            ("IncludeTags omitted", None, true),
            ("IncludeTags true", Some(true), true),
            ("IncludeTags false", Some(false), false),
        ];
        for (ask, include_tags, reported) in asks {
            let expected = if *reported { Some(&stored) } else { None };
            for (operation, uri, identifier) in [
                ("GetSink", "/GetSink", sink_arn.as_str()),
                ("GetLink", "/GetLink", link_arn.as_str()),
                ("UpdateLink", "/UpdateLink", link_arn.as_str()),
            ] {
                let case = format!("{row} read by {operation} with {ask}");
                let mut body = if operation == "UpdateLink" {
                    update_link_body(identifier, &[RESOURCE_TYPES[0]])
                } else {
                    json!({ "Identifier": identifier })
                };
                if let Some(include_tags) = include_tags {
                    body["IncludeTags"] = Value::Bool(*include_tags);
                }
                let (status, got) = call(&router, uri, &body, REGION).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {got:?}"
                );
                assert_eq!(
                    got.get("Tags"),
                    expected,
                    "case {case:?}: expected Tags to be {expected:?}, got {got:?}"
                );
            }
        }

        // And what a read reports is the very set the tagging API answers for the same
        // ARN: one stored tag set per resource, however it is reached.
        for (kind, arn) in [("sink", &sink_arn), ("link", &link_arn)] {
            let case = format!("{row} on the {kind}");
            let (status, listed) = list_tags(&router, arn, REGION).await;
            assert_eq!(
                (status, listed.get("Tags")),
                (200, Some(&tags_json(&tags))),
                "case {case:?}: expected the tag list to agree, got {status} {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_tagging_path_answers_its_own_syntax_and_not_found_refusals() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "tagging-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("a sink ARN ends in its id")
        .to_string();
    // A sink in another Region, deleted, so a case can ask about a resource that WAS
    // there.
    let dead_arn = create_sink_ok(&router, "dead-sink", OTHER_REGION).await;
    let (status, deleted) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": dead_arn.clone() }),
        OTHER_REGION,
    )
    .await;
    assert_eq!(status, 200, "DeleteSink: got {status} {deleted:?}");

    // A tag the sink really carries, so a refusal that wrote something would show.
    let seeded = tag_map(&[("real", "tag")]);
    let (status, body) = tag(&router, &sink_arn, &seeded, REGION).await;
    assert_eq!(status, 200, "seeding the sink: got {status} {body:?}");

    // Rows differ along WHY the ARN names no stored resource: an id nothing was stored
    // under, a resource since deleted, each ARN field that can fail to match
    // (partition, account, region), and a resource kind this service does not tag.
    let unknown = "11111111-2222-4333-8444-555555555555";
    let absent: Vec<(&str, String, &str)> = vec![
        (
            "a sink id nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:sink/{unknown}"),
            REGION,
        ),
        (
            "a link id nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:link/{unknown}"),
            REGION,
        ),
        ("a sink since deleted", dead_arn.clone(), OTHER_REGION),
        (
            "the stored sink at another Region's endpoint",
            sink_arn.clone(),
            OTHER_REGION,
        ),
        (
            "another account's ARN",
            format!("arn:aws:oam:{REGION}:123456789012:sink/{sink_id}"),
            REGION,
        ),
        (
            "another partition's ARN",
            format!("arn:aws-cn:oam:{REGION}:{ACCOUNT}:sink/{sink_id}"),
            REGION,
        ),
        (
            "a kind this service does not tag",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:policy/{sink_id}"),
            REGION,
        ),
    ];

    for (why, resource_arn, region) in &absent {
        // Both implemented tagging operations: the one that READS tags and the one
        // that writes them answer the same refusal for the same ARN.
        let (status, refused) = list_tags(&router, resource_arn, region).await;
        let case = format!("ListTagsForResource given {why}");
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the tagging path's not-found message, got {refused:?}"
        );

        let (status, refused) = tag(&router, resource_arn, &tag_map(&[("new", "tag")]), region)
            .await;
        let case = format!("TagResource given {why}");
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the tagging path's not-found message, got {refused:?}"
        );
    }

    // Rows differ along WHICH part of the modelled `Arn` pattern
    // `^arn:(\w|-)+:oam:.+:.+:.+` (`smithy_query(query="shape",shape="Arn")`) the value
    // misses: every one of them is the SYNTAX refusal, which echoes nothing.
    let malformed: &[(&str, &str)] = &[
        ("not an ARN at all", "not-an-oam-arn"),
        ("a bare id", "11111111-2222-4333-8444-555555555555"),
        ("another service's namespace", "arn:aws:logs:us-west-2:000000000000:sink/x"),
        ("an empty partition", "arn::oam:us-west-2:000000000000:sink/x"),
        ("too few fields", "arn:aws:oam:us-west-2:sink"),
        ("the word arn alone", "arn"),
    ];

    for (why, resource_arn) in malformed {
        for (operation, answered) in [
            ("ListTagsForResource", list_tags(&router, resource_arn, REGION).await),
            (
                "TagResource",
                tag(&router, resource_arn, &tag_map(&[("new", "tag")]), REGION).await,
            ),
        ] {
            let (status, refused) = answered;
            let case = format!("{operation} given {why}");
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {refused:?}"
            );
            assert_eq!(
                refusal_message(&refused),
                INVALID_TAGGING_ARN_MESSAGE,
                "case {case:?}: expected the ARN-syntax message, got {refused:?}"
            );
        }
    }

    // The boundary of the rule: the CONTROL PLANE answers its own modelled refusal for
    // the very resources the tagging path just reported as not found.
    for (case, uri, body, region) in [
        (
            "GetSink for the deleted sink",
            "/GetSink",
            json!({ "Identifier": dead_arn.clone() }),
            OTHER_REGION,
        ),
        (
            "GetSinkPolicy for the deleted sink",
            "/GetSinkPolicy",
            json!({ "SinkIdentifier": dead_arn.clone() }),
            OTHER_REGION,
        ),
        (
            "GetSink for a sink id nothing was stored under",
            "/GetSink",
            json!({ "Identifier": unknown }),
            REGION,
        ),
        (
            "GetLink for a link id nothing was stored under",
            "/GetLink",
            json!({ "Identifier": unknown }),
            REGION,
        ),
    ] {
        let (status, refused) = call(&router, uri, &body, region).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the control plane's not-found message, got {refused:?}"
        );
    }

    // And nothing any of the refusals carried reached either stored resource.
    let (status, listed) = list_tags(&router, &sink_arn, REGION).await;
    assert_eq!(
        (status, listed.get("Tags")),
        (200, Some(&tags_json(&seeded))),
        "the sink must still carry only what was really stored, got {status} {listed:?}"
    );
    let (status, listed) = list_tags(&router, &link_arn, REGION).await;
    assert_eq!(
        (status, listed.get("Tags")),
        (200, Some(&tags_json(&BTreeMap::new()))),
        "the link was never tagged, got {status} {listed:?}"
    );
}

/// The explicit-deny refusal `ListAttachedLinks` answers when the identifier names no
/// resource at all: the decision is reported on the sink kind of the request's Region,
/// which is the scope it was taken in.
fn expected_sink_scope_deny(region: &str) -> String {
    format!(
        "User: {} is not authorized to perform: oam:ListAttachedLinks on resource: \
         arn:aws:oam:{region}:{ACCOUNT}:sink/* with an explicit deny in a resource-based \
         policy",
        caller_principal()
    )
}

#[tokio::test]
async fn list_attached_links_decides_authorization_before_it_judges_the_identifier() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "attached-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let empty_sink = create_sink_ok(&router, "empty-sink", OTHER_REGION).await;

    // Rows differ along WHICH part of the modelled `ResourceIdentifier` pattern
    // `^[a-zA-Z0-9][a-zA-Z0-9_:\.\-\/]{0,2047}$`
    // (`smithy_query(query="shape",shape="ResourceIdentifier")`) the value misses: a
    // character no class admits, a leading character the first class excludes, the
    // empty value, and one character past the length the pattern bounds.
    let past_the_length = "s".repeat(2049);
    let longest_admitted = format!("s{}", "s".repeat(2047));
    let unnameable: Vec<(&str, String)> = vec![
        ("characters no class admits", "not!a!valid!identifier".to_string()),
        ("a space", "sink name".to_string()),
        ("a leading underscore", "_leading-underscore".to_string()),
        ("a comma", "sink,one".to_string()),
        ("the empty value", String::new()),
        ("one past the pattern's length", past_the_length),
    ];

    for (why, identifier) in &unnameable {
        let (status, refused) = call(
            &router,
            "/ListAttachedLinks",
            &json!({ "SinkIdentifier": identifier }),
            REGION,
        )
        .await;
        let case = format!("ListAttachedLinks given {why}");
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected 403 AccessDeniedException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            expected_sink_scope_deny(REGION),
            "case {case:?}: expected the explicit-deny message, got {refused:?}"
        );
    }

    // The neighbouring shapes the same rule leaves alone: a well-formed identifier
    // naming no sink is the modelled 404, an ARN of the OTHER resource kind is the
    // decision taken on THAT ARN, a stored sink with no links is an empty page, and a
    // sink with a link reports it. Each is a different answer a caller must be able to
    // tell from "you may not read this".
    let unknown = "11111111-2222-4333-8444-555555555555";
    let (status, refused) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": format!("arn:aws:oam:{REGION}:{ACCOUNT}:sink/{unknown}") }),
        REGION,
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            refusal_message(&refused)
        ),
        (
            404,
            "ResourceNotFoundException",
            RESOURCE_NOT_FOUND_MESSAGE.to_string()
        ),
        "a well-formed identifier naming no sink: got {status} {refused:?}"
    );
    // The longest identifier the pattern admits is judged by the store, not by the
    // authorization decision: it could name a sink, and it names none.
    let (status, refused) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": longest_admitted }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "the longest identifier the pattern admits: got {status} {refused:?}"
    );
    let (status, refused) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": link_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "a link ARN: got {status} {refused:?}"
    );
    assert_eq!(
        refusal_message(&refused),
        format!(
            "User: {} is not authorized to perform: oam:ListAttachedLinks on resource: \
             {link_arn} with an explicit deny in a resource-based policy",
            caller_principal()
        ),
        "a link ARN: expected the decision reported on that ARN, got {refused:?}"
    );
    let (status, listed) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": empty_sink.clone() }),
        OTHER_REGION,
    )
    .await;
    assert_eq!(
        (status, listed.get("Items")),
        (200, Some(&json!([]))),
        "a stored sink with no links: expected an empty page, got {status} {listed:?}"
    );
    assert_eq!(
        attached(&router, "the sink with a link", &sink_arn).await,
        vec![link_arn.clone()],
        "the sink with a link must still report it"
    );

    // And the members the pre-handler walk still judges are judged: `MaxResults`
    // outside its modelled range is this operation's own parameter refusal, on an
    // identifier the decision above admits.
    for (case, max_results) in [("below the minimum", 0), ("past the maximum", 1001)] {
        let (status, refused) = call(
            &router,
            "/ListAttachedLinks",
            &json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": max_results }),
            REGION,
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected 400 InvalidParameterException, got {status} {refused:?}"
        );
    }
    // A page size inside the range still pages the one link.
    let (status, listed) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": 1 }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, listed.get("Items").map(|items| items
            .as_array()
            .expect("Items is a list")
            .len())),
        (200, Some(1)),
        "MaxResults inside the range must still page, got {status} {listed:?}"
    );
}

#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_never_written() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "reserved-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let seeded = tag_map(&[("origin", "create")]);
    let (status, body) = tag(&router, &sink_arn, &seeded, REGION).await;
    assert_eq!(status, 200, "seeding the sink: got {status} {body:?}");

    // Rows differ along the CASE the prefix is spelled in and what follows it: the
    // prefix is matched case-insensitively, so every spelling is refused, and the
    // prefix alone is a key too.
    let reserved: &[(&str, &str)] = &[
        ("lower case", "aws:sim2real"),
        ("upper case", "AWS:sim2real"),
        ("title case", "Aws:cloudformation:stack-name"),
        ("mixed case", "aWs:managed"),
        ("the prefix alone", "aws:"),
        ("a reserved key with an empty value", "AWS:empty"),
    ];

    for (why, key) in reserved {
        let key = *key;
        // Every operation that SETS tags: the tagging write, and the two creates that
        // seed a resource's tags.
        let refusals: Vec<(String, u16, Value)> = vec![
            {
                let (status, body) = tag(
                    &router,
                    &sink_arn,
                    &tag_map(&[(key, "v")]),
                    REGION,
                )
                .await;
                (format!("TagResource with a key in {why}"), status, body)
            },
            {
                let (status, body) = call_as(
                    &router,
                    "/CreateSink",
                    &json!({ "Name": "reserved-create", "Tags": { key: "v" } }),
                    MONITORING_AKID,
                    OTHER_REGION,
                )
                .await;
                (format!("CreateSink with a key in {why}"), status, body)
            },
            {
                let (status, body) = call_as(
                    &router,
                    "/CreateLink",
                    &json!({
                        "LabelTemplate": "$AccountName",
                        "ResourceTypes": [RESOURCE_TYPES[0]],
                        "SinkIdentifier": sink_arn.clone(),
                        "Tags": { key: "v" },
                    }),
                    OTHER_AKID,
                    REGION,
                )
                .await;
                (format!("CreateLink with a key in {why}"), status, body)
            },
        ];
        for (case, status, refused) in refusals {
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {refused:?}"
            );
            assert_eq!(
                refusal_message(&refused),
                RESERVED_TAG_KEY_SET_MESSAGE,
                "case {case:?}: expected the reserved-namespace message, got {refused:?}"
            );
        }
    }

    // Nothing the refusals carried was written, and nothing they asked to create
    // exists: the sink carries what it carried, the link is still untagged, the sink
    // the refused create asked for is not there, and no second link was attached.
    let (status, listed) = list_tags(&router, &sink_arn, REGION).await;
    assert_eq!(
        (status, listed.get("Tags")),
        (200, Some(&tags_json(&seeded))),
        "the sink must carry only the key it was really tagged with, got {status} {listed:?}"
    );
    let (status, listed) = list_tags(&router, &link_arn, REGION).await;
    assert_eq!(
        (status, listed.get("Tags")),
        (200, Some(&tags_json(&BTreeMap::new()))),
        "the link must still be untagged, got {status} {listed:?}"
    );
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": "reserved-create" }),
        OTHER_REGION,
    )
    .await;
    assert_eq!(
        status, 404,
        "a refused CreateSink must have stored nothing, got {status} {got:?}"
    );
    assert_eq!(
        attached(&router, "after the reserved-key refusals", &sink_arn).await,
        vec![link_arn.clone()],
        "a refused CreateLink must have stored nothing"
    );

    // The boundary of the rule: a key that only LOOKS like the reserved namespace is an
    // ordinary customer key, and is stored.
    let accepted = tag_map(&[
        ("aws", "no colon"),
        ("aws-managed", "a dash, not a colon"),
        ("awsome:tag", "the prefix is longer than aws:"),
        ("cost:aws:centre", "the prefix is not at the start"),
        (" aws:leading-space", "the key does not start with it"),
    ]);
    let (status, body) = tag(&router, &sink_arn, &accepted, REGION).await;
    assert_eq!(
        status, 200,
        "keys that only look reserved must be accepted, got {status} {body:?}"
    );
    let mut expected = seeded.clone();
    expected.extend(accepted.clone());
    let (status, listed) = list_tags(&router, &sink_arn, REGION).await;
    assert_eq!(
        (status, listed.get("Tags")),
        (200, Some(&tags_json(&expected))),
        "expected the customer keys stored, got {status} {listed:?}"
    );
}
