// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the media type CloudWatch RUM announces on its responses.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response ENVELOPE are all exercised, not just a handler body.
//!
//! The rule pinned here is a property of the SERVICE, not of one operation: every
//! response RUM sends for a JSON body announces `content-type:
//! application/x-amz-json-1.1` (`crate::state::RESPONSE_CONTENT_TYPE`), whatever the
//! operation, whatever the status, and whoever produced it — a handler's success, a
//! handler's modelled error, a refusal the request boundary raised, or a scaffolded
//! operation's 501. The `restJson1` protocol the model declares announces
//! `application/json`, so this is recorded behaviour the service applies on top of the
//! codec; the BODY is the same JSON either way, which is what the second and third tests
//! check.
//!
//! The table spans what the rule generalises over rather than the two recorded steps:
//! all four HTTP methods RUM binds, every operation group (app monitor, resource policy,
//! metrics destinations, tagging, the data plane), the 200 / 400 / 404 / 409 / 501 status
//! classes, outputs that model members and outputs that model none, and both an
//! implemented operation and a scaffolded one.

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::{JSON_CONTENT_TYPE, RESPONSE_CONTENT_TYPE};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{authorization, call, create, emulator, rum_events_body, ACCOUNT, REGION};

/// What the body of an answer must be, beside the media type it announces: the gate that
/// re-announces the header may change nothing else.
#[derive(Debug, Clone, Copy)]
enum Body {
    /// A JSON object carrying at least one member.
    Members,
    /// An output shape that models no members: an empty body, or the empty JSON object.
    MemberLess,
    /// An error body, under this wire error code (restJson1 carries it as `__type`).
    Error(&'static str),
}

/// One row of the table: a request, the status it must answer, and the body it must
/// carry.
struct Case {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    expected: Body,
}

/// A well-formed IAM JSON policy, so `PutResourcePolicy` reaches its handler rather than
/// being refused on the document.
fn policy_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
            "Action": "rum:PutRumEvents",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives
/// it, percent-encoded into the ONE path segment a non-greedy `@httpLabel` occupies.
fn tags_uri(name: &str) -> String {
    let arn = format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}");
    let mut encoded = String::new();
    for byte in arn.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(*byte))
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    format!("/tags/{encoded}")
}

/// The table, in the order it must run: the rows that need the monitor `live` to exist
/// come before the row that deletes it, and the policy rows run put -> get -> delete.
///
/// `live_id` is the `Id` the seed create minted, which is how `PutRumEvents` names its
/// monitor.
fn cases(live: &str, live_id: &str) -> Vec<Case> {
    vec![
        Case {
            case: "CreateAppMonitor 200 (POST, members)",
            method: "POST",
            path: "/appmonitor".to_string(),
            body: Some(json!({"Name": "announced.second", "Domain": "example.com"})),
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "GetAppMonitor 200 (GET, members)",
            method: "GET",
            path: format!("/appmonitor/{live}"),
            body: None,
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "ListAppMonitors 200 (POST, members)",
            method: "POST",
            path: "/appmonitors".to_string(),
            body: None,
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "UpdateAppMonitor 200 (PATCH, no members)",
            method: "PATCH",
            path: format!("/appmonitor/{live}"),
            body: Some(json!({"Domain": "other.example.com"})),
            status: 200,
            expected: Body::MemberLess,
        },
        Case {
            case: "PutRumMetricsDestination 200 (POST, no members)",
            method: "POST",
            path: format!("/rummetrics/{live}/metricsdestination"),
            body: Some(json!({"Destination": "CloudWatch"})),
            status: 200,
            expected: Body::MemberLess,
        },
        Case {
            case: "ListRumMetricsDestinations 200 (GET, members)",
            method: "GET",
            path: format!("/rummetrics/{live}/metricsdestination"),
            body: None,
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "PutResourcePolicy 200 (PUT, members)",
            method: "PUT",
            path: format!("/appmonitor/{live}/policy"),
            body: Some(json!({"PolicyDocument": policy_document()})),
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "GetResourcePolicy 200 (GET, members)",
            method: "GET",
            path: format!("/appmonitor/{live}/policy"),
            body: None,
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "DeleteResourcePolicy 200 (DELETE, members)",
            method: "DELETE",
            path: format!("/appmonitor/{live}/policy"),
            body: None,
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "TagResource 204 (POST, no members)",
            method: "POST",
            path: tags_uri(live),
            body: Some(json!({"Tags": {"Team": "observability"}})),
            status: 204,
            expected: Body::MemberLess,
        },
        Case {
            case: "ListTagsForResource 200 (GET, members)",
            method: "GET",
            path: tags_uri(live),
            body: None,
            status: 200,
            expected: Body::Members,
        },
        Case {
            case: "UntagResource 204 (DELETE, no members)",
            method: "DELETE",
            path: format!("{}?tagKeys=Team", tags_uri(live)),
            body: None,
            status: 204,
            expected: Body::MemberLess,
        },
        Case {
            // The one route the protocol's routing key cannot name — a `DELETE
            // /tags/{ResourceArn}` binding no `tagKeys` value, the empty key list
            // `TagKeyList`'s `@length(min: 0)` admits — is served by the service's own
            // seam, so it must announce the same media type as every other response.
            case: "UntagResource 204 (DELETE, no tagKeys bound)",
            method: "DELETE",
            path: tags_uri(live),
            body: None,
            status: 204,
            expected: Body::MemberLess,
        },
        Case {
            case: "PutRumEvents 200 (data plane, no members)",
            method: "POST",
            path: format!("/appmonitors/{live_id}/"),
            body: Some(rum_events_body(live_id)),
            status: 200,
            expected: Body::MemberLess,
        },
        Case {
            case: "GetAppMonitor 404 (handler's modelled error)",
            method: "GET",
            path: "/appmonitor/announced.absent".to_string(),
            body: None,
            status: 404,
            expected: Body::Error("ResourceNotFoundException"),
        },
        Case {
            case: "CreateAppMonitor 409 (handler's modelled error)",
            method: "POST",
            path: "/appmonitor".to_string(),
            body: Some(json!({"Name": live, "Domain": "example.com"})),
            status: 409,
            expected: Body::Error("ConflictException"),
        },
        Case {
            case: "CreateAppMonitor 400 (boundary refusal on a modelled length)",
            method: "POST",
            path: "/appmonitor".to_string(),
            // `AppMonitorName` is `@length(min: 1, max: 255)`, so a 256-character name is
            // refused by the request boundary before any handler runs.
            body: Some(json!({"Name": "x".repeat(256), "Domain": "example.com"})),
            status: 400,
            expected: Body::Error("ValidationException"),
        },
        Case {
            case: "ListTagsForResource 400 (boundary refusal, tag frontend's code)",
            method: "GET",
            // `ResourceArn`'s pattern `arn:[^:]*:[^:]*:[^:]*:[^:]*:.*` refuses a value
            // that is not an ARN; the tagging frontend words it `BadRequestException`.
            path: "/tags/not-an-arn-at-all".to_string(),
            body: None,
            status: 400,
            expected: Body::Error("BadRequestException"),
        },
        Case {
            case: "BatchGetRumMetricDefinitions 501 (scaffolded operation)",
            method: "GET",
            path: format!("/rummetrics/{live}/metrics?destination=CloudWatch"),
            body: None,
            status: 501,
            expected: Body::Error("NotImplemented"),
        },
        Case {
            case: "DeleteAppMonitor 200 (DELETE, no members)",
            method: "DELETE",
            path: format!("/appmonitor/{live}"),
            body: None,
            status: 200,
            expected: Body::MemberLess,
        },
        Case {
            case: "DeleteAppMonitor 404 (the name it just freed)",
            method: "DELETE",
            path: format!("/appmonitor/{live}"),
            body: None,
            status: 404,
            expected: Body::Error("ResourceNotFoundException"),
        },
    ]
}

#[tokio::test]
async fn every_rum_response_announces_the_services_media_type() {
    let (router, _engine) = emulator();
    let live = "announced.monitor";
    let live_id = create(
        &router,
        "seed",
        json!({"Name": live, "Domain": "example.com"}),
    )
    .await;

    for Case {
        case,
        method,
        path,
        body,
        status,
        expected,
    } in cases(live, &live_id)
    {
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the response to announce {RESPONSE_CONTENT_TYPE:?}, got {:?}",
            reply.content_type()
        );
        // One value, not two: re-announcing the media type must REPLACE what the codec
        // stamped, since a client reading a second value would read the codec's.
        let values: Vec<&http::HeaderValue> = reply.headers.get_all("content-type").iter().collect();
        assert_eq!(
            values.len(),
            1,
            "case {case:?}: expected exactly one content-type on the response, got {values:?}"
        );
        // Only the header changes: the body is still exactly what the JSON codec wrote.
        match expected {
            Body::Members => {
                let document = reply.json(case);
                assert!(
                    document.as_object().is_some_and(|fields| !fields.is_empty()),
                    "case {case:?}: expected a JSON object carrying members, got {:?}",
                    String::from_utf8_lossy(&reply.body)
                );
            }
            Body::MemberLess => {
                assert!(
                    reply.body.is_empty()
                        || serde_json::from_slice::<Value>(&reply.body).ok() == Some(json!({})),
                    "case {case:?}: this output models no members, so the body must carry \
                     none, got {:?}",
                    String::from_utf8_lossy(&reply.body)
                );
            }
            Body::Error(code) => {
                let actual = reply.error_code(case);
                assert_eq!(
                    actual, code,
                    "case {case:?}: expected the wire error code {code:?}, got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_announced_media_type_does_not_depend_on_what_the_request_sent() {
    // The media type is a property of the RESPONSE, so it cannot be negotiated: whatever
    // media type the request declares (or declares none at all), the same JSON body comes
    // back under the same announced type. The rows span the two types a RUM client
    // plausibly sends, the awsJson types it does not, and an absent header.
    let (router, _engine) = emulator();
    let name = "negotiation.monitor";
    create(
        &router,
        "seed",
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;

    let request_types = [
        ("restJson1's own type", Some(JSON_CONTENT_TYPE)),
        ("the type RUM answers in", Some(RESPONSE_CONTENT_TYPE)),
        ("awsJson 1.0", Some("application/x-amz-json-1.0")),
        ("an unrelated type", Some("text/plain")),
        ("no content-type at all", None),
    ];
    let mut bodies: Vec<Value> = Vec::new();
    for (case, declared) in request_types {
        let reply = get_with_content_type(&router, &format!("/appmonitor/{name}"), declared).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.content_type(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the response to announce {RESPONSE_CONTENT_TYPE:?}, got {:?}",
            reply.content_type()
        );
        bodies.push(reply.json(case));
    }
    let first = bodies.first().expect("one row per request media type");
    for (index, body) in bodies.iter().enumerate() {
        assert_eq!(
            body, first,
            "case {:?}: the body must not depend on the request's media type, got {body:?} \
             against {first:?}",
            request_types[index].0
        );
    }
}

/// One answer to `GET <path>` with the request's `content-type` set to `declared`, or
/// omitted when it is `None`.
///
/// The shared `support::call` always declares `application/json`, which is exactly the
/// variable this test varies, so the request is built here instead.
async fn get_with_content_type(
    router: &MultiServiceRouter,
    path: &str,
    declared: Option<&str>,
) -> crate::support::Reply {
    let mut builder = http::Request::builder()
        .method("GET")
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
    crate::support::Reply {
        status,
        body: response.into_body(),
        headers,
    }
}

#[tokio::test]
async fn a_round_trip_still_reads_through_the_announced_media_type() {
    // The gate re-announces the header and nothing else, so a create still reads back
    // through `GetAppMonitor` member for member. Rows differ along what a create can
    // carry, so the check is not tied to one request's shape.
    let (router, _engine) = emulator();
    let rows = [
        (
            "minimal create",
            json!({"Name": "roundtrip.minimal", "Domain": "example.com"}),
            "example.com",
        ),
        (
            "create with tags and a sample rate",
            json!({
                "Name": "roundtrip-tagged",
                "Domain": "shop.example.com",
                "Tags": {"Team": "observability"},
                "AppMonitorConfiguration": {"SessionSampleRate": 0.5},
            }),
            "shop.example.com",
        ),
        (
            "create with a wildcard domain",
            json!({"Name": "roundtrip_wildcard", "Domain": "*.example.com"}),
            "*.example.com",
        ),
    ];
    for (case, body, domain) in rows {
        let name = body["Name"].as_str().expect("every row names its monitor");
        let created = call(&router, "POST", "/appmonitor", Some(body.clone())).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.content_type(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the create to announce {RESPONSE_CONTENT_TYPE:?}, got {:?}",
            created.content_type()
        );

        let read = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
        assert_eq!(
            read.content_type(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the read to announce {RESPONSE_CONTENT_TYPE:?}, got {:?}",
            read.content_type()
        );
        let monitor = read.json(case)["AppMonitor"].clone();
        assert_eq!(
            monitor["Name"].as_str(),
            Some(name),
            "case {case:?}: expected the stored monitor {name:?}, got {:?}",
            monitor["Name"]
        );
        assert_eq!(
            monitor["Domain"].as_str(),
            Some(domain),
            "case {case:?}: expected the stored Domain {domain:?}, got {:?}",
            monitor["Domain"]
        );
        assert_eq!(
            monitor["Id"].as_str(),
            created.json(case)["Id"].as_str(),
            "case {case:?}: the read must report the Id the create minted, got {:?}",
            monitor["Id"]
        );
    }
}
