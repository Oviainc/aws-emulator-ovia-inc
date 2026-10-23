// SPDX-License-Identifier: CC0-1.0

//! How this service frames an answer that carries NO body.
//!
//! RULE under test: a response this service serves with an empty body STATES its length —
//! `content-length: 0` — rather than leaving the framing header off; a response that
//! carries a body is framed from the body it carries and never claims a length that is not
//! its own. The two operations modelled with output `Unit` at HTTP 204 — `TagResource`
//! (POST `/tags/{ResourceArn}`) and `UntagResource` (DELETE `/tags/{ResourceArn}`), per
//! `smithy_query(query="operation",operation="TagResource,UntagResource")` — are the ones
//! that answer with no body, so the rule is asserted over both rather than over the one a
//! recording happened to cover.
//!
//! The rows differ along what the rule generalises over: which operation answers, which
//! resource type is tagged, how many tags the request carries, and whether the request
//! changes the stored tag set at all (a tag already present, a key the resource does not
//! carry) — the framing is a property of the RESPONSE having no body, not of the work it
//! did.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses, and what the in-process front door (the inter-service callback
//! path, the SDK connector) hands its caller unchanged. The harness is the one
//! `tests/all/channel_lifecycle.rs` owns; `dispatch` is its round trip with the response's
//! headers kept.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, dispatch, emulator, ACCOUNT, REGION};
use crate::resource_tags::urlencode;

/// The length a response with no body states.
const EMPTY_BODY_LENGTH: &str = "0";

/// The channel every origin endpoint below is packaged from.
const PARENT_CHANNEL: &str = "framing-parent-channel";

/// The two resource types this service tags.
#[derive(Clone, Copy)]
enum Kind {
    Channel,
    OriginEndpoint,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::OriginEndpoint => "origin endpoint",
        }
    }

    /// The ARN this service mints for a resource of this kind.
    fn arn(self, id: &str) -> String {
        let resource = match self {
            Self::Channel => "channels",
            Self::OriginEndpoint => "origin_endpoints",
        };
        format!("arn:aws:mediapackage:{REGION}:{ACCOUNT}:{resource}/{id}")
    }

    /// Create a resource of this kind, carrying `tags`.
    async fn create(self, router: &MultiServiceRouter, id: &str, tags: &Value) {
        let (path, body) = match self {
            Self::Channel => ("/channels", json!({ "id": id, "tags": tags })),
            Self::OriginEndpoint => (
                "/origin_endpoints",
                json!({
                    "channelId": PARENT_CHANNEL,
                    "id": id,
                    "hlsPackage": {},
                    "tags": tags
                }),
            ),
        };
        let (status, created) = call(router, "POST", path, Some(&body)).await;
        assert_eq!(
            status,
            200,
            "creating the fixture {} {id:?} expected 200, got {status} {created:?}",
            self.label()
        );
    }
}

/// `ResourceArn` is a NON-greedy `@httpLabel`, so an SDK percent-encodes the ARN's
/// reserved characters into one path segment.
fn tags_path(arn: &str) -> String {
    let label: String = arn
        .chars()
        .map(|character| match character {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect();
    format!("/tags/{label}")
}

/// Assert `response` is a bodyless answer with `expected_status`, framed with its length.
fn assert_empty_and_framed(case: &str, response: &http::Response<Bytes>, expected_status: u16) {
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

/// One tagging row: the tags the resource is created with, the tags a `TagResource` adds,
/// and the keys an `UntagResource` then removes.
struct Row {
    case: &'static str,
    created: Value,
    added: Value,
    removed: &'static [&'static str],
}

/// Rows spanning the shapes a tagging request comes in: one tag, several, a key already
/// present (so the write changes nothing), a key the resource does not carry (so the
/// removal removes nothing), keys needing percent-encoding, and the whole set removed.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one tag added, one removed",
            created: json!({ "kept": "1" }),
            added: json!({ "team": "video" }),
            removed: &["team"],
        },
        Row {
            case: "several tags added, several removed",
            created: json!({}),
            added: json!({ "team": "video", "env": "staging", "cost-centre": "1234" }),
            removed: &["env", "cost-centre"],
        },
        Row {
            case: "a tag whose key the resource already carries",
            created: json!({ "team": "audio" }),
            added: json!({ "team": "video" }),
            removed: &["team"],
        },
        Row {
            case: "a key the resource does not carry",
            created: json!({ "kept": "1" }),
            added: json!({ "team": "video" }),
            removed: &["never-set"],
        },
        Row {
            case: "keys and values needing percent-encoding",
            created: json!({ "path/key": "x" }),
            added: json!({ "with space": "a/b=c+d-e@f", "dotted.key": "1.2.3" }),
            removed: &["path/key", "with space"],
        },
        Row {
            case: "every tag removed at once",
            created: json!({ "a": "1", "b": "2" }),
            added: json!({ "c": "3" }),
            removed: &["a", "b", "c"],
        },
    ]
}

#[tokio::test]
async fn an_answer_with_no_body_states_its_length() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, row) in rows().into_iter().enumerate() {
            let case = format!("{}: {}", kind.label(), row.case);
            let id = format!("framing-{}-{index}", kind.label().replace(' ', "-"));
            kind.create(&router, &id, &row.created).await;
            let arn = kind.arn(&id);

            let tagged = dispatch(
                &router,
                "POST",
                &tags_path(&arn),
                Some(&json!({ "tags": row.added })),
            )
            .await;
            assert_empty_and_framed(&format!("{case}/TagResource"), &tagged, 204);

            let query = row
                .removed
                .iter()
                .map(|key| format!("tagKeys={}", urlencode(key)))
                .collect::<Vec<String>>()
                .join("&");
            let untagged = dispatch(
                &router,
                "DELETE",
                &format!("{}?{query}", tags_path(&arn)),
                None,
            )
            .await;
            assert_empty_and_framed(&format!("{case}/UntagResource"), &untagged, 204);

            // The 204s really did the work they report, so the framing above is that of a
            // tagging answer rather than of an answer to nothing: what is left is what
            // the two requests left.
            let (status, listed) = call(&router, "GET", &tags_path(&arn), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListTagsForResource expected 200, got {status} {listed:?}"
            );
            let mut expected = serde_json::Map::new();
            for (key, value) in row
                .created
                .as_object()
                .expect("an object of tags")
                .iter()
                .chain(row.added.as_object().expect("an object of tags").iter())
            {
                expected.insert(key.clone(), value.clone());
            }
            for key in row.removed {
                expected.remove(*key);
            }
            let reported = match listed.get("tags") {
                None | Some(Value::Null) => json!({}),
                Some(tags) => tags.clone(),
            };
            assert_eq!(
                reported,
                Value::Object(expected.clone()),
                "case {case:?}: expected the tags {:?}, got {reported:?}",
                Value::Object(expected)
            );
        }
    }
}

/// What a bodyless answer looks like on the WIRE, where the framing this service states is
/// not the last word.
///
/// The HTTP/1.1 writer this emulator is served by (hyper, inside the vendored, read-only
/// transport layer) REMOVES `content-length` from a `204`, because RFC 7230 §3.3.2 forbids
/// it there ("A server MUST NOT send a Content-Length header field in any response with a
/// status code of 1xx (Informational) or 204 (No Content)"). That is below anything this
/// crate decides, so what is pinned here is what the socket does answer — the 204, an empty
/// body and no chunked framing — while the cases above pin the framing the service itself
/// states, which is what the in-process front door (the inter-service callback path and the
/// SDK connector, both of which serve `dispatch`'s response unchanged) delivers.
#[tokio::test(flavor = "multi_thread")]
async fn a_bodyless_answer_on_the_wire_carries_the_status_and_no_body() {
    let router = emulator();
    let channel = "framing-wire-channel";
    create_channel(&router, channel).await;

    // The same router, serving the same store, on a loopback port.
    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router.clone()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let path = tags_path(&Kind::Channel.arn(channel));
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

    // And the tag really was written, so the 204 reported a tagging rather than being an
    // empty answer to nothing.
    let (status, listed) = call(&router, "GET", &tags_path(&Kind::Channel.arn(channel)), None).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource expected 200, got {status} {listed:?}"
    );
    assert_eq!(
        listed.pointer("/tags/team"),
        Some(&json!("video")),
        "the tag the wire request added must be stored, got {listed:?}"
    );
}

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
         host: localhost\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential=AKIDTEST/20260101/{REGION}/mediapackage/aws4_request, \
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
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;
    let channel = "framing-bodied-channel";

    let created = dispatch(
        &router,
        "POST",
        "/channels",
        Some(&json!({ "id": channel, "tags": { "team": "video" } })),
    )
    .await;

    // Rows: a create (200), a read (200), a listing (200), a tag listing (200), a read of
    // a resource this service does not hold (404) and a refused request (422) — a body on
    // the success path and a body on each error path.
    for (case, response) in [
        ("CreateChannel", created),
        (
            "DescribeChannel",
            dispatch(&router, "GET", &format!("/channels/{channel}"), None).await,
        ),
        ("ListChannels", dispatch(&router, "GET", "/channels", None).await),
        (
            "ListTagsForResource",
            dispatch(&router, "GET", &tags_path(&Kind::Channel.arn(channel)), None).await,
        ),
        (
            "DescribeChannel of a channel that does not exist",
            dispatch(&router, "GET", "/channels/no-such-channel", None).await,
        ),
        (
            "TagResource on an ARN that is not a MediaPackage resource",
            dispatch(
                &router,
                "POST",
                &tags_path("arn:aws:s3:::not-mediapackage"),
                Some(&json!({ "tags": { "team": "video" } })),
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
                "case {case:?}: a stated length must be the body's own, got {stated:?} for \
                 {} bytes",
                body.len()
            );
        }
    }
}
