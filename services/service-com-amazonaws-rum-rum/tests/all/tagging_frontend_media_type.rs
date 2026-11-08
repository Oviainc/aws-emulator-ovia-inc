// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the media type RUM's ARN-addressed TAGGING frontend announces.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response ENVELOPE are all exercised, not just a handler body.
//!
//! The rule pinned here: every answer of the frontend the model binds to
//! `/tags/{ResourceArn}` — `TagResource` (POST), `UntagResource` (DELETE) and
//! `ListTagsForResource` (GET), source `smithy_query(query="operation",
//! operation="TagResource,UntagResource,ListTagsForResource")` — labels its JSON body with
//! `restJson1`'s own `application/json`, whatever the status, whatever the request carried,
//! and whoever produced the answer: a handler's success, a refusal the handler worded, or a
//! refusal the request boundary raised. That is a property of the FRONTEND, so the rows span
//! its three operations, its 200 / 204 / 400 / 404 answers, requests that carry a payload
//! and requests that carry none, and the request shapes the rule generalises over (one tag,
//! several, a unicode value, the empty-string value, a key list of one, of several, and the
//! empty one the wire spells as no `tagKeys` at all).
//!
//! The last rows are the CONTRAST that keeps the rule from being read as service-wide: a
//! payload-less answer of RUM's app-monitor frontend still falls back to RUM's own JSON
//! type, `application/x-amz-json-1.1` (`crate::state::RESPONSE_CONTENT_TYPE`), which
//! [`crate::announced_media_type`] spans operation by operation.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::{JSON_CONTENT_TYPE, RESPONSE_CONTENT_TYPE};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    authorization, call, create, emulator, percent_encoded, tag_map, Reply, ACCOUNT, REGION,
};

/// The status a successful `TagResource`/`UntagResource` answers with: 204 No Content
/// (`crate::state::TAGGING_NO_CONTENT_STATUS`).
const NO_CONTENT: u16 = 204;

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives it.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// The URI the tagging operations are bound to: `/tags/{ResourceArn}`, the ARN
/// percent-encoded into the ONE path segment a non-greedy `@httpLabel` occupies.
fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", percent_encoded(resource_arn))
}

/// One row: a request, the status it must answer, the media type its answer must announce,
/// and — for a refusal — the wire error code its body must carry.
///
/// `announces` is written out per row rather than computed from the row, so the table is a
/// GOLDEN: a change in the service that moved any of them would have to move the literal
/// here too.
struct Case {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    announces: &'static str,
    error: Option<&'static str>,
}

/// A tag key the modelled `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$` refuses on its
/// negative look-ahead — the one constraint of that shape the service applies in the
/// handler, so a row naming it is refused by the FRONTEND rather than by the request
/// boundary. Source: `smithy_query(query="shape", shape="TagKey")`.
const RESERVED_KEY: &str = "aws:reserved";

/// The wire error codes this frontend words its refusals with: a request it refuses, and an
/// ARN that names no resource (`crate::state::TaggingRefusal`).
const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// The table, in the order it must run: a row that reads or removes tags comes after the
/// row that wrote them, and each mutating group addresses its OWN monitor so no row depends
/// on another's leftovers.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "ListTagsForResource 200, a monitor carrying tags (no payload)",
            method: "GET",
            path: tags_uri(&arn_of("mt-read")),
            body: None,
            status: 200,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "ListTagsForResource 200, a monitor carrying none (no payload)",
            method: "GET",
            path: tags_uri(&arn_of("mt-untagged")),
            body: None,
            status: 200,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "TagResource 204, one tag (a payload)",
            method: "POST",
            path: tags_uri(&arn_of("mt-write")),
            body: Some(json!({"Tags": {"Owner": "platform"}})),
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "TagResource 204, several tags, one of them a unicode value",
            method: "POST",
            path: tags_uri(&arn_of("mt-write")),
            body: Some(json!({"Tags": {"Team": "wërté∂", "Cost.center": "4711"}})),
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "TagResource 204, the empty string as a value",
            method: "POST",
            path: tags_uri(&arn_of("mt-write")),
            body: Some(json!({"Tags": {"Empty": ""}})),
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "UntagResource 204, one key (no payload)",
            method: "DELETE",
            path: format!("{}?tagKeys=Owner", tags_uri(&arn_of("mt-write"))),
            body: None,
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "UntagResource 204, several keys",
            method: "DELETE",
            path: format!(
                "{}?tagKeys=Team&tagKeys={}",
                tags_uri(&arn_of("mt-write")),
                percent_encoded("Cost.center")
            ),
            body: None,
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "UntagResource 204, a key the resource does not carry",
            method: "DELETE",
            path: format!("{}?tagKeys=Absent", tags_uri(&arn_of("mt-write"))),
            body: None,
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            // The one route the protocol's routing key cannot name — no `tagKeys` value at
            // all, which is how a query-bound list spells the empty list `TagKeyList`'s
            // `@length(min: 0)` admits — is served by the service's own seam, and announces
            // what the rest of its frontend announces.
            case: "UntagResource 204, no tagKeys bound at all",
            method: "DELETE",
            path: tags_uri(&arn_of("mt-write")),
            body: None,
            status: NO_CONTENT,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            // `ResourceArn`'s pattern `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*` refuses a value that
            // is not an ARN: a boundary refusal, worded by this frontend.
            case: "ListTagsForResource 400, a ResourceArn that is not an ARN",
            method: "GET",
            path: "/tags/not-an-arn".to_string(),
            body: None,
            status: 400,
            announces: JSON_CONTENT_TYPE,
            error: Some(BAD_REQUEST),
        },
        Case {
            case: "UntagResource 400, a ResourceArn missing ARN segments",
            method: "DELETE",
            path: format!("/tags/{}?tagKeys=Team", percent_encoded("arn:aws:rum")),
            body: None,
            status: 400,
            announces: JSON_CONTENT_TYPE,
            error: Some(BAD_REQUEST),
        },
        Case {
            case: "TagResource 400, a ResourceArn that is not an ARN (a payload)",
            method: "POST",
            path: "/tags/not-an-arn".to_string(),
            body: Some(json!({"Tags": {"Team": "web"}})),
            status: 400,
            announces: JSON_CONTENT_TYPE,
            error: Some(BAD_REQUEST),
        },
        Case {
            case: "UntagResource 400, a key outside TagKey's pattern (the handler's own)",
            method: "DELETE",
            path: format!(
                "{}?tagKeys={}",
                tags_uri(&arn_of("mt-read")),
                percent_encoded(RESERVED_KEY)
            ),
            body: None,
            status: 400,
            announces: JSON_CONTENT_TYPE,
            error: Some(BAD_REQUEST),
        },
        Case {
            case: "TagResource 400, a key outside TagKey's pattern (the handler's own)",
            method: "POST",
            path: tags_uri(&arn_of("mt-read")),
            body: Some(json!({"Tags": {RESERVED_KEY: "x"}})),
            status: 400,
            announces: JSON_CONTENT_TYPE,
            error: Some(BAD_REQUEST),
        },
        Case {
            case: "ListTagsForResource 404, a well-formed ARN naming no resource",
            method: "GET",
            path: tags_uri(&arn_of("mt-absent")),
            body: None,
            status: 404,
            announces: JSON_CONTENT_TYPE,
            error: Some(NOT_FOUND),
        },
        Case {
            case: "UntagResource 404, a well-formed ARN naming no resource",
            method: "DELETE",
            path: format!("{}?tagKeys=Team", tags_uri(&arn_of("mt-absent"))),
            body: None,
            status: 404,
            announces: JSON_CONTENT_TYPE,
            error: Some(NOT_FOUND),
        },
        Case {
            case: "TagResource 404, an ARN of another account (a payload)",
            method: "POST",
            path: tags_uri("arn:aws:rum:us-east-1:111122223333:appmonitor/mt-read"),
            body: Some(json!({"Tags": {"Team": "web"}})),
            status: 404,
            announces: JSON_CONTENT_TYPE,
            error: Some(NOT_FOUND),
        },
        // The contrast: RUM's app-monitor frontend, which answers a payload-less request in
        // RUM's own JSON type and a payload-carrying one in the type that payload declared.
        Case {
            case: "GetAppMonitor 200 (another frontend, no payload)",
            method: "GET",
            path: "/appmonitor/mt-read".to_string(),
            body: None,
            status: 200,
            announces: RESPONSE_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "GetAppMonitor 404 (another frontend, no payload)",
            method: "GET",
            path: "/appmonitor/mt-absent".to_string(),
            body: None,
            status: 404,
            announces: RESPONSE_CONTENT_TYPE,
            error: Some("ResourceNotFoundException"),
        },
        Case {
            case: "CreateAppMonitor 200 (another frontend, a payload)",
            method: "POST",
            path: "/appmonitor".to_string(),
            body: Some(json!({"Name": "mt-created", "Domain": "example.com"})),
            status: 200,
            announces: JSON_CONTENT_TYPE,
            error: None,
        },
        Case {
            case: "DeleteAppMonitor 200 (another frontend, no payload)",
            method: "DELETE",
            path: "/appmonitor/mt-created".to_string(),
            body: None,
            status: 200,
            announces: RESPONSE_CONTENT_TYPE,
            error: None,
        },
    ]
}

/// The monitors the table addresses, each with the tags it starts out carrying.
async fn seed(router: &MultiServiceRouter) {
    create(
        router,
        "seed mt-read",
        json!({"Name": "mt-read", "Domain": "example.com", "Tags": {"Team": "web"}}),
    )
    .await;
    create(
        router,
        "seed mt-untagged",
        json!({"Name": "mt-untagged", "Domain": "example.com"}),
    )
    .await;
    create(
        router,
        "seed mt-write",
        json!({"Name": "mt-write", "Domain": "example.com"}),
    )
    .await;
}

/// The media type the answer announces, asserted as the ONE value a client reads.
fn announces(reply: &Reply, case: &str, want: &str) {
    assert_eq!(
        reply.content_type(),
        Some(want),
        "case {case:?}: expected the answer to announce {want:?}, got {:?}",
        reply.content_type()
    );
    let values: Vec<&http::HeaderValue> = reply.headers.get_all("content-type").iter().collect();
    assert_eq!(
        values.len(),
        1,
        "case {case:?}: expected exactly one content-type on the answer, got {values:?}"
    );
}

#[tokio::test]
async fn every_tagging_answer_announces_the_rest_json_media_type() {
    let (router, _engine) = emulator();
    seed(&router).await;

    for Case {
        case,
        method,
        path,
        body,
        status,
        announces: want_type,
        error,
    } in cases()
    {
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        announces(&reply, case, want_type);
        // Only the header is at stake: the body is still exactly what the JSON codec wrote.
        match error {
            Some(code) => {
                let actual = reply.error_code(case);
                assert_eq!(
                    actual, code,
                    "case {case:?}: expected the wire error code {code:?}, got {actual:?}"
                );
            }
            None if status == NO_CONTENT => assert!(
                reply.body.is_empty()
                    || serde_json::from_slice::<Value>(&reply.body).ok() == Some(json!({})),
                "case {case:?}: this output models no members, so the body must carry none, \
                 got {:?}",
                String::from_utf8_lossy(&reply.body)
            ),
            None => {
                let document = reply.json(case);
                assert!(
                    document.as_object().is_some(),
                    "case {case:?}: expected a JSON object, got {:?}",
                    String::from_utf8_lossy(&reply.body)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_tagging_answer_announces_the_same_type_whatever_the_request_declared() {
    // The frontend's media type is its own, so it does not depend on the `content-type` the
    // request happens to send — nor on whether it sends one. The rows span the two types a
    // RUM client plausibly sends, the awsJson type RUM's other frontends answer in, an
    // unrelated type, and an absent header, on a GET and on a DELETE of this frontend.
    let (router, _engine) = emulator();
    seed(&router).await;

    let declared_types = [
        ("restJson1's own type", Some(JSON_CONTENT_TYPE)),
        ("the type RUM's other frontends answer in", Some(RESPONSE_CONTENT_TYPE)),
        ("an unrelated type", Some("text/plain")),
        ("no content-type at all", None),
    ];
    let read = tags_uri(&arn_of("mt-read"));
    let removal = format!("{}?tagKeys=Absent", tags_uri(&arn_of("mt-write")));
    let mut bodies: Vec<Value> = Vec::new();
    for (declared_case, declared) in declared_types {
        for (method, path, status) in [
            ("GET", read.as_str(), 200),
            ("DELETE", removal.as_str(), NO_CONTENT),
        ] {
            let case = format!("{method} with {declared_case}");
            let reply = send(&router, method, path, declared).await;
            assert_eq!(
                reply.status, status,
                "case {case:?}: expected HTTP {status}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            announces(&reply, &case, JSON_CONTENT_TYPE);
            if method == "GET" {
                bodies.push(reply.json(&case));
            }
        }
    }
    let first = bodies.first().expect("one read per declared media type").clone();
    for (index, document) in bodies.iter().enumerate() {
        assert_eq!(
            *document, first,
            "case {:?}: the body must not depend on the request's media type, got \
             {document:?} against {first:?}",
            declared_types[index].0
        );
    }
}

/// One answer to a payload-less `method path` whose request declares `declared` as its
/// `content-type`, or omits the header when it is `None`.
///
/// The shared `support::call` always declares `application/json`, which is exactly the
/// variable this suite varies, so the request is built here instead.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    declared: Option<&str>,
) -> Reply {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://rum.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization(REGION));
    if let Some(value) = declared {
        builder = builder.header("content-type", value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    Reply {
        status,
        body: response.into_body(),
        headers,
    }
}

/// One row of the round-trip table: the tags a write carries, and the map a read must then
/// report.
struct Trip {
    case: &'static str,
    tags: Value,
    expected: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn tags_still_round_trip_through_the_announced_media_type() {
    // The frontend settles the header and nothing else, so a write still reads back map for
    // map. The rows differ along what a tag map can carry, so the check is not tied to one
    // request's shape.
    let (router, _engine) = emulator();
    let rows = [
        Trip {
            case: "one tag",
            tags: json!({"Tags": {"Team": "web"}}),
            expected: &[("Team", "web")],
        },
        Trip {
            case: "several tags, one a unicode value",
            tags: json!({"Tags": {"Team": "wërté∂", "Cost.center": "4711"}}),
            expected: &[("Cost.center", "4711"), ("Team", "wërté∂")],
        },
        Trip {
            case: "the empty string as a value",
            tags: json!({"Tags": {"Empty": ""}}),
            expected: &[("Empty", "")],
        },
    ];
    for (
        index,
        Trip {
            case,
            tags: body,
            expected,
        },
    ) in rows.into_iter().enumerate()
    {
        let monitor = format!("mt-trip-{index}");
        create(
            &router,
            case,
            json!({"Name": monitor, "Domain": "example.com"}),
        )
        .await;
        let uri = tags_uri(&arn_of(&monitor));

        let written = call(&router, "POST", &uri, Some(body)).await;
        assert_eq!(
            written.status, NO_CONTENT,
            "case {case:?}: expected TagResource to answer {NO_CONTENT}, got {} {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        announces(&written, case, JSON_CONTENT_TYPE);

        let read = call(&router, "GET", &uri, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        announces(&read, case, JSON_CONTENT_TYPE);
        let reported = read.json(case);
        let want: BTreeMap<String, String> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let actual: BTreeMap<String, String> =
            serde_json::from_value(tag_map(&reported)).unwrap_or_else(|e| {
                panic!("case {case:?}: expected a tag map, got {reported:?} ({e})")
            });
        assert_eq!(
            actual, want,
            "case {case:?}: expected the tags the write stored {want:?}, got {actual:?}"
        );
        assert_eq!(
            reported["ResourceArn"].as_str(),
            Some(arn_of(&monitor).as_str()),
            "case {case:?}: expected the resource's own ARN, got {:?}",
            reported["ResourceArn"]
        );

        let removed = call(
            &router,
            "DELETE",
            &format!(
                "{uri}?{}",
                expected
                    .iter()
                    .map(|(key, _)| format!("tagKeys={}", percent_encoded(key)))
                    .collect::<Vec<String>>()
                    .join("&")
            ),
            None,
        )
        .await;
        assert_eq!(
            removed.status, NO_CONTENT,
            "case {case:?}: expected UntagResource to answer {NO_CONTENT}, got {} {:?}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        announces(&removed, case, JSON_CONTENT_TYPE);

        let after = call(&router, "GET", &uri, None).await;
        announces(&after, case, JSON_CONTENT_TYPE);
        assert_eq!(
            tag_map(&after.json(case)),
            json!({}),
            "case {case:?}: expected every named key removed, got {:?}",
            String::from_utf8_lossy(&after.body)
        );
    }
}
