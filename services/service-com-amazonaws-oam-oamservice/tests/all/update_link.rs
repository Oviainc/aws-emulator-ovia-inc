// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateLink`.
//!
//! The rules under test:
//!
//! * `UpdateLink` STATES what a link shares rather than adding to it — "your input
//!   here replaces the current set of data types that are shared" — so the stored
//!   `ResourceTypes` and `LinkConfiguration` become exactly what the request carried,
//!   and a later `GetLink` reads that back.
//! * The authorization decision on the link `Identifier` names comes FIRST: a request
//!   naming no link this caller owns earns the same explicit-deny refusal (HTTP 403
//!   `AccessDeniedException`) whatever else the request carries — a well-formed
//!   `ResourceTypes`, an empty one, one past the modelled bounds, or none at all — so
//!   a caller probing permissions learns the authorization outcome and nothing about
//!   what is stored.
//! * On a link the caller DOES own, the modelled bounds of `ResourceTypes` are then
//!   reported: an absent member as `MissingRequiredParameterException`, a value outside
//!   `@length(1..50)` / `@uniqueItems` as `InvalidParameterException`, and an element
//!   outside the `ResourceType` enum as the request front end's `BadRequestException`
//!   — and a refused update changes nothing.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers), so what is asserted
//! is what a signed AWS SDK request observes.

use serde_json::{json, Map, Value};
use service_com_amazonaws_oam_oamservice::state::{
    caller_principal, MAX_RESOURCE_TYPES, MIN_RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::sinks_and_links::{
    call, call_as, create_link_ok, create_sink_ok, emulator, error_code, field, ACCOUNT, AKID,
    RESOURCE_TYPES,
};

/// The region every case signs for unless it is asking about another endpoint.
const REGION: &str = "us-west-2";

/// A second region, so a case can name a link that exists at another endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// A second access key id: a link belongs to the source account that created it, which
/// this service reads from the request's credential, so this is a DIFFERENT source
/// account from [`AKID`].
const OTHER_AKID: &str = "AKIDOAMTEST2";

/// One row of the "what else the request carries" table: a body builder that takes the
/// identifier under test.
type BodyShape = (&'static str, Box<dyn Fn(&str) -> Value>);

/// The explicit-deny refusal `UpdateLink` answers.
///
/// Its decision is taken on the link resource KIND of the request's Region and
/// account, not on one named link, so the refusal names that kind's wildcard ARN
/// whatever identifier was supplied.
fn expected_deny(region: &str) -> String {
    format!(
        "User: {} is not authorized to perform: oam:UpdateLink on resource: \
         arn:aws:oam:{region}:{ACCOUNT}:link/* with an explicit deny in a resource-based policy",
        caller_principal()
    )
}

/// The `message` member of a refusal, whichever casing the answering shape models it
/// with.
fn refusal_message(body: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    panic!("expected a message member, got {body:?}")
}

/// The `ResourceTypes` a response reports, in the order it reported them.
fn reported_types(case: &str, body: &Value) -> Vec<String> {
    body.get("ResourceTypes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceTypes member, got {body:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string, got {body:?}"))
                .to_string()
        })
        .collect()
}

/// `UpdateLink` with the members `body` carries.
async fn update(
    router: &MultiServiceRouter,
    body: &Value,
    akid: &str,
    region: &str,
) -> (u16, Value) {
    call_as(router, "/UpdateLink", body, akid, region).await
}

/// An `UpdateLink` body naming `identifier` and `resource_types`.
fn update_body(identifier: &str, resource_types: &[&str]) -> Value {
    json!({ "Identifier": identifier, "ResourceTypes": resource_types })
}

#[tokio::test]
async fn update_link_replaces_what_the_link_shares_and_get_link_reads_it_back() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let link_id = link_arn
        .rsplit('/')
        .next()
        .expect("a link ARN ends in its id")
        .to_string();

    // Rows differ along what the rule generalises over: how many types are shared
    // (one, several, every modelled value, each published end of the 1..50 bound),
    // whether the new set grows, shrinks or is disjoint from the stored one, and the
    // order the request lists them in. The link starts out sharing exactly
    // `RESOURCE_TYPES[0]` (`create_link_ok`).
    let every_type: Vec<&str> = RESOURCE_TYPES.to_vec();
    let reversed: Vec<&str> = RESOURCE_TYPES.iter().rev().copied().collect();
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("a disjoint single type", vec![RESOURCE_TYPES[1]]),
        ("a larger set", vec![RESOURCE_TYPES[1], RESOURCE_TYPES[2]]),
        ("every modelled type", every_type.clone()),
        ("the same types in the other order", reversed.clone()),
        (
            "back down to the minimum the list admits",
            vec![RESOURCE_TYPES[6]],
        ),
        ("the type it started with", vec![RESOURCE_TYPES[0]]),
    ];

    for (case, resource_types) in &cases {
        assert!(
            resource_types.len() >= MIN_RESOURCE_TYPES
                && resource_types.len() <= MAX_RESOURCE_TYPES,
            "case {case:?}: the row must be inside the modelled 1..50 bound, got {}",
            resource_types.len()
        );
        let (status, updated) = update(
            &router,
            &update_body(&link_arn, resource_types),
            AKID,
            REGION,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateLink, got {status} {updated:?}"
        );
        // Reported from the stored record: the ARN, id, label and sink are the link's
        // own and cannot be changed by an update.
        assert_eq!(
            (
                field(case, &updated, "Arn"),
                field(case, &updated, "Id"),
                field(case, &updated, "SinkArn"),
                field(case, &updated, "Label"),
                field(case, &updated, "LabelTemplate"),
            ),
            (
                link_arn.clone(),
                link_id.clone(),
                sink_arn.clone(),
                ACCOUNT.to_string(),
                "$AccountName".to_string(),
            ),
            "case {case:?}: expected the stored link's own members, got {updated:?}"
        );
        assert_eq!(
            reported_types(case, &updated),
            *resource_types,
            "case {case:?}: expected exactly the types the request named, got {updated:?}"
        );

        // The reader answers from the same stored record — the update REPLACED the
        // list rather than merging into it.
        let (status, got) = call(
            &router,
            "/GetLink",
            &json!({ "Identifier": link_arn.clone() }),
            REGION,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetLink, got {status} {got:?}"
        );
        assert_eq!(
            reported_types(case, &got),
            *resource_types,
            "case {case:?}: GetLink must read back exactly what the update stored, got {got:?}"
        );
    }

    // `LinkConfiguration` is stated the same way: the request says what the link's
    // filters now are, so one that carries none leaves the link with none.
    let filtered = json!({
        "Identifier": link_arn.clone(),
        "ResourceTypes": [RESOURCE_TYPES[1], RESOURCE_TYPES[0]],
        "LinkConfiguration": {
            "LogGroupConfiguration": { "Filter": "LogGroupName LIKE 'aws/lambda/%'" },
            "MetricConfiguration": { "Filter": "Namespace = 'AWS/EC2'" },
        },
    });
    let (status, updated) = update(&router, &filtered, AKID, REGION).await;
    assert_eq!(
        status, 200,
        "with filters: expected 200, got {status} {updated:?}"
    );
    assert_eq!(
        updated.get("LinkConfiguration"),
        filtered.get("LinkConfiguration"),
        "with filters: expected the filters the request carried, got {updated:?}"
    );
    let (status, got) = call(
        &router,
        "/GetLink",
        &json!({ "Identifier": link_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, got.get("LinkConfiguration")),
        (200, filtered.get("LinkConfiguration")),
        "with filters: GetLink must read the stored filters back, got {status} {got:?}"
    );

    let (status, updated) = update(
        &router,
        &update_body(&link_arn, &[RESOURCE_TYPES[0]]),
        AKID,
        REGION,
    )
    .await;
    assert_eq!(
        (status, updated.get("LinkConfiguration")),
        (200, None),
        "without filters: expected the filters to be replaced by none, got {status} {updated:?}"
    );
    let (status, got) = call(
        &router,
        "/GetLink",
        &json!({ "Identifier": link_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, got.get("LinkConfiguration")),
        (200, None),
        "without filters: GetLink must read none back, got {status} {got:?}"
    );

    // `IncludeTags` suppresses the link's tags rather than asking for them, and an
    // untagged link reports the EMPTY set it holds — the same set the tag list answers
    // for it — rather than leaving the member out.
    let mut with_tags = update_body(&link_arn, &[RESOURCE_TYPES[0]]);
    with_tags["IncludeTags"] = Value::Bool(true);
    let (status, updated) = update(&router, &with_tags, AKID, REGION).await;
    assert_eq!(
        (status, updated.get("Tags")),
        (200, Some(&json!({}))),
        "an untagged link reports its empty tag set when asked, got {status} {updated:?}"
    );
    with_tags["IncludeTags"] = Value::Bool(false);
    let (status, updated) = update(&router, &with_tags, AKID, REGION).await;
    assert_eq!(
        (status, updated.get("Tags")),
        (200, None),
        "IncludeTags false leaves the member out, got {status} {updated:?}"
    );

    // Every identifier form the modelled `ResourceIdentifier` admits for a link — its
    // ARN and its bare id — reaches the same record.
    for identifier in [link_arn.as_str(), link_id.as_str()] {
        let (status, updated) = update(
            &router,
            &update_body(identifier, &[RESOURCE_TYPES[2]]),
            AKID,
            REGION,
        )
        .await;
        assert_eq!(
            (status, field("identifier form", &updated, "Arn")),
            (200, link_arn.clone()),
            "via {identifier:?}: expected the one stored link, got {status} {updated:?}"
        );
    }
}

#[tokio::test]
async fn update_link_decides_authorization_before_it_judges_the_rest_of_the_request() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", REGION).await;
    let mine = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    // A second source account's link to the same sink, and a link at another Region's
    // endpoint: neither is a link this caller may update.
    let another_accounts = create_link_ok(&router, &sink_arn, OTHER_AKID, REGION).await;
    let other_region_sink = create_sink_ok(&router, "other-sink", OTHER_REGION).await;
    let other_region_link = create_link_ok(&router, &other_region_sink, AKID, OTHER_REGION).await;
    let unknown = "11111111-2222-3333-4444-555555555555";

    // Rows differ along WHY the identifier names no link this caller owns here.
    let identifiers: Vec<(&str, String)> = vec![
        (
            "a link ARN nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:link/{unknown}"),
        ),
        ("a bare id nothing was stored under", unknown.to_string()),
        (
            "a link ARN in another account",
            format!("arn:aws:oam:{REGION}:123456789012:link/{unknown}"),
        ),
        (
            "another source account's link",
            another_accounts.clone(),
        ),
        (
            "a link at another Region's endpoint",
            other_region_link.clone(),
        ),
        (
            "a sink ARN, which is the other resource kind",
            sink_arn.clone(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:logs:{REGION}:{ACCOUNT}:link/{unknown}"),
        ),
    ];

    // And rows along what else the request carries: the decision above must not
    // depend on any of it. Each of these would otherwise be a different refusal —
    // an absent required member, a list below or past its modelled length, a repeated
    // element, a value outside the modelled enum.
    let too_many: Vec<String> = (0..=MAX_RESOURCE_TYPES)
        .map(|index| RESOURCE_TYPES[index % RESOURCE_TYPES.len()].to_string())
        .collect();
    let shapes: Vec<BodyShape> = vec![
        (
            "a well-formed ResourceTypes",
            Box::new(|identifier: &str| update_body(identifier, &[RESOURCE_TYPES[0]])),
        ),
        (
            "no ResourceTypes at all",
            Box::new(|identifier: &str| json!({ "Identifier": identifier })),
        ),
        (
            "an empty ResourceTypes",
            Box::new(|identifier: &str| update_body(identifier, &[])),
        ),
        (
            "a repeated resource type",
            Box::new(|identifier: &str| {
                update_body(identifier, &[RESOURCE_TYPES[0], RESOURCE_TYPES[0]])
            }),
        ),
        (
            "a resource type the model does not declare",
            Box::new(|identifier: &str| update_body(identifier, &["AWS::Logs::LogStream"])),
        ),
        (
            "more resource types than the list admits",
            Box::new(move |identifier: &str| {
                json!({ "Identifier": identifier, "ResourceTypes": too_many.clone() })
            }),
        ),
        (
            "every other member as well",
            Box::new(|identifier: &str| {
                json!({
                    "Identifier": identifier,
                    "ResourceTypes": [RESOURCE_TYPES[1]],
                    "IncludeTags": true,
                    "LinkConfiguration": {
                        "MetricConfiguration": { "Filter": "Namespace = 'AWS/EC2'" },
                    },
                })
            }),
        ),
    ];

    for (why, identifier) in &identifiers {
        for (shape, build) in &shapes {
            let case = format!("{why} with {shape}");
            let (status, refused) = update(&router, &build(identifier), AKID, REGION).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {refused:?}"
            );
            assert_eq!(
                refusal_message(&refused),
                expected_deny(REGION),
                "case {case:?}: expected the explicit-deny message, got {refused:?}"
            );
        }
    }

    // Nothing any of those refusals named was changed: the other account's link, and
    // the one at the other endpoint, still share what they were created with.
    for (case, identifier, region) in [
        ("another account's link", another_accounts.as_str(), REGION),
        (
            "the link at the other endpoint",
            other_region_link.as_str(),
            OTHER_REGION,
        ),
        ("this caller's own link", mine.as_str(), REGION),
    ] {
        let (status, got) = call(
            &router,
            "/GetLink",
            &json!({ "Identifier": identifier }),
            region,
        )
        .await;
        assert_eq!(
            (status, reported_types(case, &got)),
            (200, vec![RESOURCE_TYPES[0].to_string()]),
            "case {case:?}: expected the link unchanged, got {status} {got:?}"
        );
    }
}

#[tokio::test]
async fn update_link_reports_the_modelled_bounds_on_a_link_the_caller_owns() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;

    // Rows differ along which modelled rule of `ResourceTypes` the request misses:
    // its presence, each end of `@length(1..50)`, `@uniqueItems`, and the
    // `ResourceType` enum its elements target — one row per rule, plus a value the
    // issue did not cite for the enum and the exact bound for the length. Enum
    // MEMBERSHIP is a rule of the request body, so it is the front end's
    // `BadRequestException`; the rest are this operation's own modelled parameter
    // errors.
    let too_many: Vec<String> = (0..=MAX_RESOURCE_TYPES)
        .map(|index| format!("{}{index}", RESOURCE_TYPES[0]))
        .collect();
    let mut cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no ResourceTypes at all",
            json!({ "Identifier": link_arn.clone() }),
            400,
            "MissingRequiredParameterException",
        ),
        (
            "an empty ResourceTypes",
            update_body(&link_arn, &[]),
            400,
            "InvalidParameterException",
        ),
        (
            "one resource type named twice",
            update_body(&link_arn, &[RESOURCE_TYPES[0], RESOURCE_TYPES[0]]),
            400,
            "InvalidParameterException",
        ),
        (
            "a resource type the model does not declare",
            update_body(&link_arn, &["AWS::Logs::LogStream"]),
            400,
            "BadRequestException",
        ),
        (
            "a modelled resource type in the wrong case",
            update_body(&link_arn, &["aws::logs::loggroup"]),
            400,
            "BadRequestException",
        ),
        (
            "more resource types than the list admits",
            json!({ "Identifier": link_arn.clone(), "ResourceTypes": too_many }),
            400,
            "InvalidParameterException",
        ),
    ];
    // The `Identifier` is still judged by the modelled pattern before any of this:
    // a value the pattern does not admit never names a resource to decide about.
    cases.push((
        "an Identifier outside the modelled pattern",
        update_body("link one", &[RESOURCE_TYPES[0]]),
        400,
        "BadRequestException",
    ));

    for (case, body, expected_status, expected_code) in &cases {
        let (status, refused) = update(&router, body, AKID, REGION).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {refused:?}"
        );
        // A refused update stores nothing: the link still shares what it was created
        // with.
        let (status, got) = call(
            &router,
            "/GetLink",
            &json!({ "Identifier": link_arn.clone() }),
            REGION,
        )
        .await;
        assert_eq!(
            (status, reported_types(case, &got)),
            (200, vec![RESOURCE_TYPES[0].to_string()]),
            "case {case:?}: expected the link unchanged after the refusal, got {status} {got:?}"
        );
    }

    // The exact ends of the modelled length are admitted: one type, and as many as
    // the list bounds.
    for (case, count) in [
        ("the minimum the list admits", MIN_RESOURCE_TYPES),
        ("as many types as the modelled catalog holds", RESOURCE_TYPES.len()),
    ] {
        let types: Vec<&str> = RESOURCE_TYPES[..count].to_vec();
        let (status, updated) = update(&router, &update_body(&link_arn, &types), AKID, REGION).await;
        assert_eq!(
            (status, reported_types(case, &updated)),
            (
                200,
                types.iter().map(|t| (*t).to_string()).collect::<Vec<String>>()
            ),
            "case {case:?}: expected the update to be stored, got {status} {updated:?}"
        );
    }

    // And a link whose sink is gone with it: deleting the link makes the identifier
    // name no link this caller owns, which is the authorization refusal again.
    let (status, _) = call(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": link_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(status, 200, "DeleteLink must succeed, got {status}");
    let mut body = Map::new();
    body.insert("Identifier".to_string(), Value::String(link_arn.clone()));
    let (status, refused) = update(&router, &Value::Object(body), AKID, REGION).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "a deleted link: expected 403 AccessDeniedException, got {status} {refused:?}"
    );
}
