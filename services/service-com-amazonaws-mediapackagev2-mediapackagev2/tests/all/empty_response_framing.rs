// SPDX-License-Identifier: CC0-1.0

//! How this service frames an answer that carries NO body.
//!
//! RULE under test: a response this service serves with an empty body STATES its length —
//! `content-length: 0` — rather than leaving the framing header off; a response that
//! carries a body is framed from the body it carries and never claims a length that is not
//! its own. The two operations modelled with output `Unit` at HTTP 204 — `TagResource`
//! (POST `/tags/{ResourceArn}`) and `UntagResource` (DELETE `/tags/{ResourceArn}`), per
//! `smithy_query(query="operation",operation="TagResource,UntagResource",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")`
//! — are the ones that answer with no body, so the rule is asserted over both rather than
//! over the one a recording happened to cover.
//!
//! The rows differ along what the rule generalises over: which operation answers, which
//! resource type is tagged (every level of this service's hierarchy, read off that
//! resource's own `Arn`), how many tags the request carries, and whether the request
//! changes the stored tag set at all (a key already present, a key the resource does not
//! carry) — the framing is a property of the RESPONSE having no body, not of the work it
//! did.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses, and what the in-process front door (the inter-service callback
//! path, the SDK connector) hands its caller unchanged. The client is the one
//! `tests/all/channel_lifecycle.rs` owns; `dispatch` is its round trip with the response's
//! headers kept.

use serde_json::{json, Value};

use crate::channel_lifecycle::{minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{encoded, epoch_seconds_from_now, harvest_body};

/// The length a response with no body states.
const EMPTY_BODY_LENGTH: &str = "0";

/// The channel group every resource below hangs under.
const GROUP: &str = "framing-group";

/// The channel the origin endpoint and the harvest job belong to.
const CHANNEL: &str = "framing-channel";

/// The origin endpoint the harvest job exports.
const ENDPOINT: &str = "framing-endpoint";

/// The manifest the endpoint publishes, which the harvest job harvests.
const MANIFEST: &str = "index";

/// The URI the tagging operations bind. `ResourceArn` is a NON-greedy `@httpLabel`, so an
/// SDK percent-encodes the ARN's reserved characters into one path segment; the escaper is
/// the one this crate's suites share.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", encoded(arn))
}

/// The `Arn` a create reported, which is how every case addresses the resource it made.
fn arn_of(case: &str, body: &Value) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a create reports the resource's Arn, got {body}"))
        .to_string()
}

/// Create one resource of every level this service tags, answering `(label, ARN)` for each.
///
/// The ARNs are the service's own, read off each create's response, so the rows below tag
/// exactly what the service minted rather than an ARN this file spelled.
async fn taggable_resources(emulator: &Emulator) -> Vec<(&'static str, String)> {
    let (status, group) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "setup: CreateChannelGroup got {status} {group}");
    let (status, channel) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(status, 200, "setup: CreateChannel got {status} {channel}");
    let (status, endpoint) = emulator
        .create_endpoint(GROUP, CHANNEL, ENDPOINT, minimal_endpoint("TS"))
        .await;
    assert_eq!(
        status, 200,
        "setup: CreateOriginEndpoint got {status} {endpoint}"
    );
    let start = epoch_seconds_from_now(600.0);
    let mut submission = harvest_body(start, start + 60.0, &[MANIFEST]);
    submission["HarvestJobName"] = json!("framing-job");
    let (status, job) = emulator
        .call(
            "POST",
            &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob"),
            Some(submission),
        )
        .await;
    assert_eq!(status, 200, "setup: CreateHarvestJob got {status} {job}");

    vec![
        ("channel group", arn_of("channel group", &group)),
        ("channel", arn_of("channel", &channel)),
        ("origin endpoint", arn_of("origin endpoint", &endpoint)),
        ("harvest job", arn_of("harvest job", &job)),
    ]
}

/// Assert `response` is a bodyless answer with `expected_status`, framed with its length.
fn assert_empty_and_framed(
    case: &str,
    response: &http::Response<bytes::Bytes>,
    expected_status: u16,
) {
    let status = response.status().as_u16();
    let body = response.body();
    assert_eq!(
        status,
        expected_status,
        "case {case:?}: expected {expected_status}, got {status} {:?}",
        String::from_utf8_lossy(body)
    );
    assert!(
        body.is_empty(),
        "case {case:?}: this answer carries no body, got {:?}",
        String::from_utf8_lossy(body)
    );
    let stated = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok());
    assert_eq!(
        stated,
        Some(EMPTY_BODY_LENGTH),
        "case {case:?}: an answer with no body must state its length, got {stated:?}"
    );
}

/// One tagging row: the tags a `TagResource` adds, and the keys an `UntagResource` then
/// removes.
struct Row {
    case: &'static str,
    added: &'static [(&'static str, &'static str)],
    removed: &'static [&'static str],
}

/// Rows spanning the shapes a tagging request comes in: one pair, several, a key the
/// previous row already wrote (so the write changes nothing), a key the resource does not
/// carry (so the removal removes nothing), values needing percent-encoding, and every key
/// written so far removed at once.
const ROWS: &[Row] = &[
    Row {
        case: "one tag added, one removed",
        added: &[("team", "video")],
        removed: &["team"],
    },
    Row {
        case: "several tags added, some removed",
        added: &[("team", "video"), ("env", "staging"), ("cost-centre", "1234")],
        removed: &["env", "cost-centre"],
    },
    Row {
        case: "a key the resource already carries",
        added: &[("team", "audio")],
        removed: &[],
    },
    Row {
        case: "a key the resource does not carry",
        added: &[("kept", "1")],
        removed: &["never-set"],
    },
    Row {
        case: "keys and values needing percent-encoding",
        added: &[("with space", "a/b=c+d-e@f"), ("dotted.key", "1.2.3")],
        removed: &["dotted.key"],
    },
    Row {
        case: "every key written so far removed at once",
        added: &[("last", "one")],
        removed: &["team", "kept", "last"],
    },
];

/// The tag map `ListTagsForResource` reports, as an order-independent map.
fn listed_tags(case: &str, body: &Value) -> std::collections::BTreeMap<String, String> {
    match body.get("tags") {
        None | Some(Value::Null) => std::collections::BTreeMap::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: tags must be a map, got {other:?}"),
    }
}

#[tokio::test]
async fn an_answer_with_no_body_states_its_length() {
    let emulator = Emulator::new();

    for (kind, arn) in taggable_resources(&emulator).await {
        // What the resource carries before the rows run: a create may have tagged it, and
        // the rows below only state what they themselves write.
        let (status, listed) = emulator.call("GET", &tags_path(&arn), None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: ListTagsForResource got {status} {listed}"
        );
        let mut expected = listed_tags(kind, &listed);

        for row in ROWS {
            let case = format!("{kind}: {}", row.case);
            let tags: serde_json::Map<String, Value> = row
                .added
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            let tagged = emulator
                .dispatch(
                    "POST",
                    &tags_path(&arn),
                    Some(json!({ "tags": Value::Object(tags) })),
                )
                .await;
            assert_empty_and_framed(&format!("{case}/TagResource"), &tagged, 204);
            for (key, value) in row.added {
                expected.insert((*key).to_string(), (*value).to_string());
            }

            let query = row
                .removed
                .iter()
                .map(|key| format!("tagKeys={}", encoded(key)))
                .collect::<Vec<String>>()
                .join("&");
            let path = if query.is_empty() {
                // `TagKeys` is `@required`, so a removal always names at least one key;
                // this row names one the resource does not carry instead of none.
                format!("{}?tagKeys=absent-key", tags_path(&arn))
            } else {
                format!("{}?{query}", tags_path(&arn))
            };
            let untagged = emulator.dispatch("DELETE", &path, None).await;
            assert_empty_and_framed(&format!("{case}/UntagResource"), &untagged, 204);
            for key in row.removed {
                expected.remove(*key);
            }

            // The 204s really did the work they report, so the framing above is that of a
            // tagging answer rather than of an answer to nothing: what is left is what the
            // two requests left.
            let (status, listed) = emulator.call("GET", &tags_path(&arn), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource got {status} {listed}"
            );
            assert_eq!(
                listed_tags(&case, &listed),
                expected,
                "case {case:?}: expected the tags {expected:?}, got {listed}"
            );
        }
    }
}

/// What a bodyless answer looks like on the WIRE, where the framing this service states is
/// not the last word.
///
/// The HTTP/1.1 writer this emulator is served by (hyper, inside the vendored, read-only
/// transport layer — see `vendor/server/src/lib.rs`, whose own framing helper skips a `204`
/// for the same reason) does not write `content-length` on a `204`, because RFC 7230 §3.3.2
/// forbids it there ("A server MUST NOT send a Content-Length header field in any response
/// with a status code of 1xx (Informational) or 204 (No Content)"). That is below anything
/// this crate decides, so what is pinned here is what the socket does answer — the 204, an
/// empty body and no chunked framing — while the cases above pin the framing the service
/// itself states, which is what the in-process front door (the inter-service callback path
/// and the SDK connector, both of which serve the router's response unchanged) delivers.
#[tokio::test(flavor = "multi_thread")]
async fn a_bodyless_answer_on_the_wire_carries_the_status_and_no_body() {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(engine),
    );

    // The group whose ARN is tagged, created through the same router.
    let create = http::Request::builder()
        .method("POST")
        .uri("/channelGroup")
        .header("host", format!("mediapackagev2.{WIRE_REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{WIRE_REGION}/mediapackagev2/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            json!({"ChannelGroupName": "framing-wire-group"}).to_string(),
        ))
        .expect("a well-formed test request");
    let created = router.dispatch(create).await;
    assert_eq!(
        created.status().as_u16(),
        200,
        "setup: CreateChannelGroup got {}",
        String::from_utf8_lossy(created.body())
    );
    let created: Value =
        serde_json::from_slice(created.body()).expect("a CreateChannelGroup body is JSON");
    let arn = arn_of("the wire group", &created);

    // The same router, serving the same store, on a loopback port.
    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router.clone()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let path = tags_path(&arn);
    let answer = tokio::task::spawn_blocking(move || {
        raw_request(port, "POST", &path, r#"{"tags":{"team":"video"}}"#)
    })
    .await
    .expect("the socket exchange runs to completion");
    assert!(
        answer.starts_with("HTTP/1.1 204"),
        "expected a 204 from TagResource on the wire, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case TagResource: a 204 carries no body, got {body:?}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case TagResource: a bodyless answer is not chunked, got {head:?}"
    );
}

/// The Region the wire case signs for.
const WIRE_REGION: &str = "us-west-2";

/// One request written straight onto a socket, with everything the server answered read
/// back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the framing this very case is about.
fn raw_request(port: u16, method: &str, path: &str, body: &str) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         host: mediapackagev2.{WIRE_REGION}.amazonaws.com\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential=AKIDTEST/20260101/{WIRE_REGION}/mediapackagev2/aws4_request, \
         SignedHeaders=host, Signature=abc\r\n\
         content-type: application/json\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("read the whole answer");
    String::from_utf8_lossy(&answer).to_string()
}

/// RULE: an answer that carries a body is framed from the body it carries — on the success
/// path and on the error path alike — so a stated length is never a length of something
/// else.
#[tokio::test]
async fn an_answer_with_a_body_is_framed_from_the_body_it_carries() {
    let emulator = Emulator::new();
    let resources = taggable_resources(&emulator).await;
    let group_arn = resources
        .first()
        .map(|(_, arn)| arn.clone())
        .unwrap_or_default();

    // Rows: a read, a listing, a tag listing, a read of a resource this service does not
    // hold (404) and a refused request (400) — a body on the success path and a body on
    // each error path.
    for (case, response) in [
        (
            "GetChannelGroup",
            emulator
                .dispatch("GET", &format!("/channelGroup/{GROUP}"), None)
                .await,
        ),
        (
            "ListChannels",
            emulator
                .dispatch("GET", &format!("/channelGroup/{GROUP}/channel"), None)
                .await,
        ),
        (
            "ListTagsForResource",
            emulator.dispatch("GET", &tags_path(&group_arn), None).await,
        ),
        (
            "GetChannelGroup of a group that does not exist",
            emulator
                .dispatch("GET", "/channelGroup/no-such-group", None)
                .await,
        ),
        (
            "TagResource on an ARN that is not a MediaPackageV2 resource",
            emulator
                .dispatch(
                    "POST",
                    &tags_path("arn:aws:s3:::not-mediapackagev2"),
                    Some(json!({ "tags": { "team": "video" } })),
                )
                .await,
        ),
    ] {
        let body = response.body();
        assert!(
            !body.is_empty(),
            "case {case:?}: this answer carries a body, got {} with none",
            response.status()
        );
        if let Some(stated) = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
        {
            assert_eq!(
                stated,
                body.len().to_string(),
                "case {case:?}: a stated length must be the body's own, got {stated:?} for {} \
                 bytes",
                body.len()
            );
        }
    }
}
