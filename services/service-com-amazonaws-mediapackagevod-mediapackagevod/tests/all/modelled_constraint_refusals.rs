// SPDX-License-Identifier: CC0-1.0

//! How MediaPackage VOD refuses a request that breaks a rule its own MODEL declares.
//!
//! The rule under test, stated once: a request whose input members do not satisfy the
//! model — a value outside a declared `@range`, or a `@required` member the request does
//! not carry — is refused exactly like a rule one of this service's handlers checks
//! itself. HTTP **422**, `x-amzn-errortype` (and the body's `__type`)
//! `UnprocessableEntityException`, and both body members: a `message` of the form
//! `<member_name_in_snake_case> <phrase>` and an `errors` map from the offending member's
//! WIRE name to the phrases it violated. A numeric bound stated at both ends is reported
//! as the whole published range, whichever end the value fell outside.
//!
//! Each test is a TABLE, and the rows span what the rule generalises over rather than one
//! recorded call: every listing that binds a page size and values on both sides of each
//! published bound (plus each bound itself and a value inside, which must be ACCEPTED),
//! and every `@required` member of every operation whose route a request carrying no such
//! member still resolves — in the body of each create and in the body of `TagResource`.
//!
//! `UntagResource$TagKeys` is the one `@required` member no row can leave out: it is
//! `@httpQuery("tagKeys")`, and the framework's REST route table treats a `@required`
//! request-observable binding as part of route MATCHING
//! (`vendor/smithy-schema-server/src/rest_xml.rs`, `RequiredBinding` and `route_lookup`),
//! so a `DELETE /tags/{ResourceArn}` carrying no `tagKeys` names no modelled operation at
//! all and is answered by the routing-error path, before any input exists to validate.
//! That is route matching below this crate rather than the envelope this suite pins.
//!
//! Every case drives real HTTP through the router the production binary builds. The
//! harness (`router`, `call`, `tags_route`, the creates) is the one
//! `tests/all/resource_tag_listing.rs` defines, and `errors_of` the one
//! `tests/all/tag_restrictions.rs` defines, so this suite adds no second copy of either.

use serde_json::{json, Value};

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::resource_tag_listing::{
    call, create_parent_group, router, tags_route, Answer, PARENT_GROUP, REGION, SOURCE_ARN,
};
use crate::tag_restrictions::errors_of;

/// The modelled refusal of an invalid request parameter
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`), with the status that shape declares.
const UNPROCESSABLE: (u16, &str) = (422, "UnprocessableEntityException");

/// The `message` member of a refusal body (the modelled `Message`, `jsonName: message`),
/// or `"<unset>"` when the body carries none.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<unset>")
        .to_string()
}

/// The error shape the body itself names (restJson1's `__type`).
fn json_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<unset>")
        .to_string()
}

/// Assert one refusal states itself as this service's parameter violation: the modelled
/// status, the modelled code in BOTH places a restJson1 client resolves the shape from,
/// the `errors` attribution, and the one-line `message` that states the same attribution.
fn assert_member_violation(answer: &Answer, case: &str, member: &str, phrase: &str) {
    let (status, code) = UNPROCESSABLE;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.errortype, code,
        "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
        answer.errortype
    );
    assert_eq!(
        json_type(&answer.body),
        code,
        "case {case:?}: the body's __type must name the same shape as the header, got {:?}",
        json_type(&answer.body)
    );
    let errors = errors_of(&answer.body);
    assert_eq!(
        errors,
        [(member.to_string(), vec![phrase.to_string()])]
            .into_iter()
            .collect(),
        "case {case:?}: expected the violation attributed to {member:?}, got {errors:?}"
    );
    assert_eq!(
        message_of(&answer.body),
        format!("{} {phrase}", snake_case(member)),
        "case {case:?}: expected the member-and-constraint message, got {:?}",
        message_of(&answer.body)
    );
}

/// A wire member name as the `message` spells it: `maxResults` -> `max_results`. The
/// expectation is built here rather than written out per row, so every row states the one
/// form.
fn snake_case(member: &str) -> String {
    let mut out = String::with_capacity(member.len() + 4);
    for character in member.chars() {
        if character.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// `body` without its `member`: the request a row sends, built from the request that
/// carries every required member so the two differ in exactly that one.
fn without(body: &Value, member: &str) -> Value {
    let mut body = body.clone();
    if let Value::Object(members) = &mut body {
        members.remove(member);
    }
    body
}

/// `body` with its `id` member replaced, so each accepted row creates its own resource
/// rather than re-using an id an earlier row already stored.
fn with_id(body: &Value, id: &str) -> Value {
    let mut body = body.clone();
    body["id"] = json!(id);
    body
}

/// Every listing that binds a page size, by the route it answers on.
///
/// All three declare `MaxResults` as `@httpQuery("maxResults")` targeting the `MaxResults`
/// shape, which carries `@range(min: 1, max: 1000)`
/// (`smithy_query(query="operation",operation="ListAssets,ListPackagingConfigurations,ListPackagingGroups")`).
const PAGED_LISTINGS: [(&str, &str); 3] = [
    ("ListAssets", "/assets"),
    ("ListPackagingConfigurations", "/packaging_configurations"),
    ("ListPackagingGroups", "/packaging_groups"),
];

/// A page size outside the modelled range is a violation of the member it arrived in, on
/// every listing that binds one — and the refusal states the whole published bound.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_a_member_violation() {
    let router = router();
    // The bound as the model publishes it, read off the service's own constants rather
    // than written out, so the phrase cannot drift from the range enforced.
    let phrase = vod::between_phrase(vod::MIN_PAGE_SIZE, vod::MAX_PAGE_SIZE);

    // One past each published bound, and values far outside on both sides: the rule is
    // about the bound, not about a particular number.
    let refused = [
        ("just below the minimum", "0"),
        ("negative", "-1"),
        ("far below the minimum", "-2147483648"),
        ("just above the maximum", "1001"),
        ("far above the maximum", "2147483647"),
    ];
    for (operation, route) in PAGED_LISTINGS {
        for (why, value) in refused {
            let case = format!("{operation} maxResults={value} ({why})");
            let answer = call(
                &router,
                REGION,
                "GET",
                &format!("{route}?maxResults={value}"),
                None,
            )
            .await;
            assert_member_violation(&answer, &case, vod::PAGE_SIZE_QUERY, &phrase);
        }
    }

    // Not a blanket refusal: each published bound itself, a value between them, and no
    // page size at all are all served.
    let accepted = [
        ("the minimum", Some("1")),
        ("the maximum", Some("1000")),
        ("between the bounds", Some("500")),
        ("no page size at all", None),
    ];
    for (operation, route) in PAGED_LISTINGS {
        for (why, value) in accepted {
            let case = format!("{operation} {why}");
            let path = match value {
                Some(value) => format!("{route}?maxResults={value}"),
                None => route.to_string(),
            };
            let answer = call(&router, REGION, "GET", &path, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an in-bound page size must be served, got {} body {}",
                answer.status, answer.body
            );
        }
    }
}

/// One absent-member row: the request that leaves a `@required` member out, and the
/// request that is the same but for carrying it.
struct Absent {
    case: String,
    method: &'static str,
    route: String,
    sent: Option<Value>,
    /// The wire name of the `@required` member `sent` does not carry.
    member: &'static str,
    carried_route: String,
    carried: Option<Value>,
    accepted_status: u16,
}

/// A `@required` member the request does not carry is a violation of that member, on
/// every operation that declares one — and the same request carrying it is served.
///
/// `smithy_query(query="operation",operation="CreateAsset,CreatePackagingConfiguration,CreatePackagingGroup,TagResource")`
/// gives the required members the rows span: `Id` on all three creates, `PackagingGroupId`
/// on the asset and the configuration, `SourceArn` and `SourceRoleArn` on the asset, and
/// `Tags` on `TagResource`. Each row omits exactly ONE of them and carries every other, so
/// the refusal names the member the row is about.
#[tokio::test]
async fn an_absent_required_member_is_a_member_violation() {
    let router = router();
    create_parent_group(&router, REGION).await;

    let asset = json!({
        "id": "vod-asset",
        "packagingGroupId": PARENT_GROUP,
        "sourceArn": SOURCE_ARN,
        "sourceRoleArn": format!(
            "arn:aws:iam::{}:role/MediaPackageVod",
            engine::emulator_account_id()
        ),
    });
    let configuration = json!({
        "id": "vod-configuration",
        "packagingGroupId": PARENT_GROUP,
        "hlsPackage": { "hlsManifests": [{}] },
    });
    let group = json!({ "id": "vod-group" });
    // The tag rows address a resource that EXISTS, so their refusal can only be about the
    // member they leave out.
    let tags = tags_route(&vod::resource_arn(
        vod::ARN_KIND_PACKAGING_GROUPS,
        REGION,
        PARENT_GROUP,
    ));

    let mut rows: Vec<Absent> = Vec::new();
    // (the create's route, the request that carries every required member, the status the
    // create is answered with, the members it declares required)
    let creates: [(&str, &Value, u16, &[&str]); 3] = [
        ("/packaging_groups", &group, 200, &["id"]),
        (
            "/assets",
            &asset,
            200,
            &["id", "packagingGroupId", "sourceArn", "sourceRoleArn"],
        ),
        (
            "/packaging_configurations",
            &configuration,
            202,
            &["id", "packagingGroupId"],
        ),
    ];
    for (route, complete, accepted_status, members) in creates {
        for member in members {
            rows.push(Absent {
                case: format!("POST {route} with no {member}"),
                method: "POST",
                route: route.to_string(),
                sent: Some(without(complete, member)),
                member,
                carried_route: route.to_string(),
                // A distinct id per row: the accepted request STORES a resource, and the
                // same id twice would be refused as a duplicate rather than served.
                carried: Some(with_id(complete, &format!("accepted-{}", snake_case(member)))),
                accepted_status,
            });
        }
    }
    rows.push(Absent {
        case: "POST /tags/{ResourceArn} with no tags".to_string(),
        method: "POST",
        route: tags.clone(),
        sent: Some(json!({})),
        member: "tags",
        carried_route: tags.clone(),
        carried: Some(json!({ "tags": { "environment": "production" } })),
        accepted_status: 204,
    });

    for row in rows {
        let answer = call(&router, REGION, row.method, &row.route, row.sent).await;
        assert_member_violation(&answer, &row.case, row.member, vod::NO_VALUE_PHRASE);

        // Not a blanket refusal: the same request carrying that member is served.
        let answer = call(&router, REGION, row.method, &row.carried_route, row.carried).await;
        assert_eq!(
            answer.status, row.accepted_status,
            "case {:?}: the same request carrying {:?} must be served with {}, got {} body {}",
            row.case, row.member, row.accepted_status, answer.status, answer.body
        );
    }
}

/// A member the request leaves out and one it sends empty are the same claim about the
/// same member, so they are refused identically — the check in front of the operation and
/// the one inside it answer the same envelope.
#[tokio::test]
async fn an_absent_member_and_an_empty_member_are_refused_alike() {
    let router = router();
    // (case, the body)
    let bodies = [
        ("no id member at all", json!({})),
        ("an id sent as the empty string", json!({ "id": "" })),
    ];
    // (case, status, errortype, message, errors) of each answer, compared as a whole.
    let mut answered: Vec<(&str, u16, String, String, String)> = Vec::new();
    for (case, body) in bodies {
        let answer = call(&router, REGION, "POST", "/packaging_groups", Some(body)).await;
        assert_member_violation(&answer, case, "id", vod::NO_VALUE_PHRASE);
        answered.push((
            case,
            answer.status,
            answer.errortype.clone(),
            message_of(&answer.body),
            format!("{:?}", errors_of(&answer.body)),
        ));
    }
    let (first, rest) = answered.split_first().expect("every body was answered");
    for other in rest {
        assert_eq!(
            (other.1, &other.2, &other.3, &other.4),
            (first.1, &first.2, &first.3, &first.4),
            "case {:?}: must be answered exactly as case {:?} is",
            other.0,
            first.0
        );
    }
}

/// The refusal body carries the two members this service's validation envelope documents
/// and nothing else of its own, so a client reads both from one answer.
#[tokio::test]
async fn the_refusal_body_carries_both_documented_members() {
    let router = router();
    let answer = call(
        &router,
        REGION,
        "GET",
        &format!("/assets?maxResults={}", vod::MAX_PAGE_SIZE + 1),
        None,
    )
    .await;
    let (status, code) = UNPROCESSABLE;
    assert_eq!(
        answer.status, status,
        "expected HTTP {status}, got {} body {}",
        answer.status, answer.body
    );
    let members: Vec<String> = answer
        .body
        .as_object()
        .map(|body| body.keys().cloned().collect())
        .unwrap_or_default();
    for member in ["message", "errors"] {
        assert!(
            members.iter().any(|name| name == member),
            "expected the body to carry {member:?}, got {members:?} in {}",
            answer.body
        );
    }
    assert_eq!(
        json_type(&answer.body),
        code,
        "expected the body's __type to name {code}, got {:?}",
        json_type(&answer.body)
    );
}

/// What stands in front of every operation does not change what an accepted request
/// answers: a listing still reports what the store holds, at the page size asked for.
#[tokio::test]
async fn an_accepted_request_is_served_by_the_operation_unchanged() {
    let router = router();
    let stored = ["group-a", "group-b", "group-c"];
    for id in stored {
        let answer = call(
            &router,
            REGION,
            "POST",
            "/packaging_groups",
            Some(json!({ "id": id })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup: CreatePackagingGroup {id:?}: body {}",
            answer.body
        );
    }

    // (case, the page size asked for, how many of the stored groups one page carries)
    let pages: [(&str, usize, usize); 4] = [
        ("one per page", 1, 1),
        ("two per page", 2, 2),
        ("exactly as many as are stored", stored.len(), stored.len()),
        ("more than the store holds", vod::MAX_PAGE_SIZE, stored.len()),
    ];
    for (case, size, expected) in pages {
        let answer = call(
            &router,
            REGION,
            "GET",
            &format!("/packaging_groups?maxResults={size}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            answer.status, answer.body
        );
        let listed = answer
            .body
            .get("packagingGroups")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected} packaging groups on the page, got {listed} in {}",
            answer.body
        );
    }
}
