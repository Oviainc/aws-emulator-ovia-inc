// SPDX-License-Identifier: CC0-1.0

//! What a tag write leaves on the `lastUpdatedAt` of the tunnel it changes.
//!
//! RULE: a successful call that changes something a tunnel reports leaves the
//! record's `lastUpdatedAt` ("The last time the tunnel was updated", the
//! modelled `Tunnel$lastUpdatedAt` documentation) at the instant of that
//! change, and a call that changes nothing leaves every stamp exactly as it
//! was. A tunnel's tag set is something it reports — `DescribeTunnel` returns
//! it inside the same `Tunnel` shape as the stamp — so a `TagResource` or
//! `UntagResource` that changes the stored set is an update of the tunnel, and
//! one that does not is not.
//!
//! The table spans what the rule generalises over: which operation writes
//! (`TagResource`, `UntagResource`), how many keys the request names, how big
//! the stored set was before it, whether the request actually changes that set
//! (a new key, a new value for a stored key, the identical pair again, a key the
//! tunnel never carried, a mix of both), and the tunnel's status, since a closed
//! tunnel is still a tagged resource. Whether the stamp must move is not a
//! column: it is derived from whether the stored set changed, which is the rule
//! itself.
//!
//! Each row rewinds the tunnel's whole timeline before the write, so a stamp
//! left at the instant of the change is plainly distinguishable from the one the
//! previous call wrote whatever the clock's resolution.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// How far the timeline of a tunnel is moved back before the write under test.
const REWIND: Duration = Duration::from_secs(120);

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// What a client reads back about one tunnel through `DescribeTunnel`.
#[derive(Debug, Clone, PartialEq)]
struct Reported {
    status: Option<String>,
    created_at: Option<f64>,
    last_updated_at: Option<f64>,
    source_last_updated_at: Option<f64>,
    destination_last_updated_at: Option<f64>,
    tags: BTreeMap<String, String>,
}

/// A modelled `TagList` as a key -> value map.
fn tag_map(tags: Option<&Value>) -> BTreeMap<String, String> {
    tags.and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|tag| {
                    Some((
                        tag["key"].as_str()?.to_string(),
                        tag["value"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `DescribeTunnel`'s report of `tunnel_id`, or a panic naming `case`.
async fn reported(router: &MultiServiceRouter, tunnel_id: &str, case: &str) -> Reported {
    let (status, described) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTunnel answered {status} {described:?}"
    );
    let tunnel = &described["tunnel"];
    Reported {
        status: tunnel["status"].as_str().map(str::to_string),
        created_at: tunnel["createdAt"].as_f64(),
        last_updated_at: tunnel["lastUpdatedAt"].as_f64(),
        source_last_updated_at: tunnel["sourceConnectionState"]["lastUpdatedAt"].as_f64(),
        destination_last_updated_at: tunnel["destinationConnectionState"]["lastUpdatedAt"].as_f64(),
        tags: tag_map(tunnel.get("tags")),
    }
}

/// The `lastUpdatedAt` `ListTunnels` reports for `tunnel_id`, so the other
/// reader of the same record can be held to the same stamp.
async fn listed_last_updated_at(router: &MultiServiceRouter, tunnel_id: &str) -> Option<f64> {
    let (_, listed) = call(router, "ListTunnels", &json!({})).await;
    listed["tunnelSummaries"]
        .as_array()?
        .iter()
        .find(|summary| summary["tunnelId"].as_str() == Some(tunnel_id))
        .and_then(|summary| summary["lastUpdatedAt"].as_f64())
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn listed_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = call(router, "ListTagsForResource", &json!({"resourceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource answered {status} {listed:?}"
    );
    tag_map(listed.get("tags"))
}

/// The stored `lastUpdatedAt` of `tunnel_id`.
fn stored_stamp(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str, case: &str) -> SystemTime {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| tunnel.last_updated_at)
        .unwrap_or_else(|| panic!("case {case:?}: tunnel {tunnel_id} is not in the store"))
}

/// Move the whole timeline of `tunnel_id` back by [`REWIND`], so every stamp the
/// record carries is in the past and a stamp written by the call under test is
/// unmistakable.
fn rewind(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str, case: &str) {
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let tunnel = store
        .tunnel_mut(REGION, tunnel_id)
        .unwrap_or_else(|| panic!("case {case:?}: the opened tunnel was not persisted"));
    tunnel.created_at -= REWIND;
    tunnel.last_updated_at -= REWIND;
    tunnel.expires_at -= REWIND;
    tunnel.source_connection_state.last_updated_at -= REWIND;
    tunnel.destination_connection_state.last_updated_at -= REWIND;
}

/// The status the tunnel is in when the write under test arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Open,
    Closed,
}

impl Status {
    /// The value `DescribeTunnel` reports for a tunnel in this status.
    fn wire(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::Closed => "CLOSED",
        }
    }
}

/// The tag write under test.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// `TagResource` with these key/value pairs.
    Tag(&'static [(&'static str, &'static str)]),
    /// `UntagResource` with these keys.
    Untag(&'static [&'static str]),
}

impl Write {
    /// The operation name and request body for `arn`.
    fn request(self, arn: &str) -> (&'static str, Value) {
        match self {
            Self::Tag(tags) => (
                "TagResource",
                json!({
                    "resourceArn": arn,
                    "tags": tags
                        .iter()
                        .map(|(key, value)| json!({"key": key, "value": value}))
                        .collect::<Vec<Value>>(),
                }),
            ),
            Self::Untag(keys) => (
                "UntagResource",
                json!({"resourceArn": arn, "tagKeys": keys}),
            ),
        }
    }
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    /// The tags the tunnel is opened with.
    initial: &'static [(&'static str, &'static str)],
    status: Status,
    write: Write,
    /// The tag set the tunnel must carry afterwards.
    expected: &'static [(&'static str, &'static str)],
}

/// A `(key, value)` slice as the map the service reports.
fn expected_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The rows: one per shape of tag write, over both statuses a tagged tunnel can
/// be in.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a first key on a tunnel carrying none",
            initial: &[],
            status: Status::Open,
            write: Write::Tag(&[("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "one new key beside two stored ones",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Tag(&[("stage", "beta")]),
            expected: &[("env", "test"), ("owner", "team"), ("stage", "beta")],
        },
        Case {
            label: "a new value for a key the tunnel already carries",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "prod")]),
            expected: &[("env", "prod")],
        },
        Case {
            label: "three keys at once: two new and one rewritten unchanged",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "test"), ("x", "1"), ("y", "2")]),
            expected: &[("env", "test"), ("x", "1"), ("y", "2")],
        },
        Case {
            label: "the one stored pair written again, identically",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Tag(&[("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "every stored pair written again, identically",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Tag(&[("owner", "team"), ("env", "test")]),
            expected: &[("env", "test"), ("owner", "team")],
        },
        Case {
            label: "one of two keys removed",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Untag(&["env"]),
            expected: &[("owner", "team")],
        },
        Case {
            label: "every key removed at once",
            initial: &[("env", "test"), ("owner", "team")],
            status: Status::Open,
            write: Write::Untag(&["owner", "env"]),
            expected: &[],
        },
        Case {
            label: "a stored key and an absent one removed together",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Untag(&["env", "never-set"]),
            expected: &[],
        },
        Case {
            label: "only keys the tunnel never carried",
            initial: &[("env", "test")],
            status: Status::Open,
            write: Write::Untag(&["never-set", "nor-this"]),
            expected: &[("env", "test")],
        },
        Case {
            label: "a removal from a tunnel carrying no tags at all",
            initial: &[],
            status: Status::Open,
            write: Write::Untag(&["env"]),
            expected: &[],
        },
        Case {
            label: "a first key on a CLOSED tunnel",
            initial: &[],
            status: Status::Closed,
            write: Write::Tag(&[("env", "test")]),
            expected: &[("env", "test")],
        },
        Case {
            label: "a key removed from a CLOSED tunnel",
            initial: &[("env", "test")],
            status: Status::Closed,
            write: Write::Untag(&["env"]),
            expected: &[],
        },
        Case {
            label: "a key the CLOSED tunnel never carried",
            initial: &[("env", "test")],
            status: Status::Closed,
            write: Write::Untag(&["never-set"]),
            expected: &[("env", "test")],
        },
    ]
}

/// Open one tunnel in the state `case` asks for and return `(tunnelId, arn)`.
async fn tunnel_for(router: &MultiServiceRouter, case: &Case) -> (String, String) {
    let label = case.label;
    let mut input = json!({
        "description": label,
        "destinationConfig": {"thingName": "thing-stamped", "services": ["SSH"]},
    });
    if !case.initial.is_empty() {
        // `TagList` is `length(min: 1, max: 200)`, so an empty set is sent as no
        // member at all rather than as an empty list.
        input["tags"] = json!(case
            .initial
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect::<Vec<Value>>());
    }
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(
        status, 200,
        "case {label:?}: OpenTunnel answered {status} {opened:?}"
    );
    let tunnel_id = opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no tunnelId in {opened:?}"))
        .to_string();
    let arn = opened["tunnelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no tunnelArn in {opened:?}"))
        .to_string();

    if case.status == Status::Closed {
        let (status, closed) = call(router, "CloseTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CloseTunnel answered {status} {closed:?}"
        );
    }
    (tunnel_id, arn)
}

#[tokio::test]
async fn a_tag_write_stamps_the_tunnel_exactly_when_it_changes_what_it_reports() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for case in cases() {
        let label = case.label;
        let (tunnel_id, arn) = tunnel_for(&router, &case).await;
        // Every stamp the record carries goes into the past, so a stamp written
        // by the call under test cannot be confused with an earlier one.
        rewind(&engine, &tunnel_id, label);

        let before = reported(&router, &tunnel_id, label).await;
        let before_stamp = stored_stamp(&engine, &tunnel_id, label);
        assert_eq!(
            before.status.as_deref(),
            Some(case.status.wire()),
            "case {label:?}: expected the tunnel {} before the write, got {before:?}",
            case.status.wire()
        );
        assert_eq!(
            before.tags,
            expected_map(case.initial),
            "case {label:?}: expected the opened tags {:?}, got {:?}",
            expected_map(case.initial),
            before.tags
        );

        let expected = expected_map(case.expected);
        // The rule's condition: the stamp moves exactly when the set the tunnel
        // reports is not the set it reported before.
        let changes = expected != before.tags;

        let (operation, input) = case.write.request(&arn);
        let sent = SystemTime::now();
        let (status, body) = call(&router, operation, &input).await;
        let answered = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} answered {status} {body:?}"
        );

        let after = reported(&router, &tunnel_id, label).await;
        assert_eq!(
            after.tags, expected,
            "case {label:?}: expected the tags {expected:?}, got {:?}",
            after.tags
        );
        assert_eq!(
            listed_tags(&router, &arn, label).await,
            expected,
            "case {label:?}: ListTagsForResource must report the same set as DescribeTunnel"
        );

        let after_stamp = stored_stamp(&engine, &tunnel_id, label);
        if changes {
            assert!(
                after_stamp >= sent && after_stamp <= answered,
                "case {label:?}: a write that changed the tag set must stamp lastUpdatedAt with \
                 the instant of the change, expected a stamp in {:?}..={:?}, got {after_stamp:?}",
                sent,
                answered
            );
            assert!(
                after.last_updated_at > before.last_updated_at,
                "case {label:?}: the tunnel must report the new stamp, expected one later than \
                 {:?}, got {:?}",
                before.last_updated_at,
                after.last_updated_at
            );
        } else {
            assert_eq!(
                after_stamp, before_stamp,
                "case {label:?}: a write that changed nothing must leave lastUpdatedAt alone, \
                 expected {before_stamp:?}, got {after_stamp:?}"
            );
            assert_eq!(
                after.last_updated_at, before.last_updated_at,
                "case {label:?}: the reported stamp must not move, expected {:?}, got {:?}",
                before.last_updated_at, after.last_updated_at
            );
        }

        // A tag write touches nothing else the tunnel reports: not when it was
        // created, not its status, and neither end's connection state — those
        // stamps belong to the connection transitions.
        assert_eq!(
            (
                after.created_at,
                after.status.as_deref(),
                after.source_last_updated_at,
                after.destination_last_updated_at
            ),
            (
                before.created_at,
                before.status.as_deref(),
                before.source_last_updated_at,
                before.destination_last_updated_at
            ),
            "case {label:?}: a tag write must leave createdAt, status and both connection \
             states alone, expected {before:?}, got {after:?}"
        );
        // Both readers of the record report one stamp.
        assert_eq!(
            listed_last_updated_at(&router, &tunnel_id).await,
            after.last_updated_at,
            "case {label:?}: ListTunnels must report the same lastUpdatedAt as DescribeTunnel, \
             got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_tag_write_stamps_nothing() {
    // The control for the table: the stamp follows a change the call actually
    // made, so a request the service refuses must leave the record exactly as it
    // was — including its `lastUpdatedAt`.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let case = Case {
        label: "the refused writes",
        initial: &[("env", "test")],
        status: Status::Open,
        write: Write::Tag(&[]),
        expected: &[],
    };
    let (tunnel_id, arn) = tunnel_for(&router, &case).await;
    rewind(&engine, &tunnel_id, case.label);

    let refusals: Vec<(&str, Value)> = vec![
        // A reserved tag key, which neither tag path may write.
        (
            "TagResource",
            json!({"resourceArn": arn, "tags": [{"key": "aws:owner", "value": "x"}]}),
        ),
        (
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": ["aws:owner", "env"]}),
        ),
        // An ARN of a tunnel this endpoint does not hold.
        (
            "TagResource",
            json!({
                "resourceArn": format!("{arn}-missing"),
                "tags": [{"key": "env", "value": "other"}],
            }),
        ),
        (
            "UntagResource",
            json!({"resourceArn": format!("{arn}-missing"), "tagKeys": ["env"]}),
        ),
    ];

    for (operation, input) in refusals {
        let label = format!("{operation} refused with {input}");
        let before = reported(&router, &tunnel_id, &label).await;
        let before_stamp = stored_stamp(&engine, &tunnel_id, &label);

        let (status, body) = call(&router, operation, &input).await;
        assert!(
            status >= 400,
            "case {label:?}: the request must be refused, answered {status} {body:?}"
        );

        let after = reported(&router, &tunnel_id, &label).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused write must change nothing the tunnel reports, \
             expected {before:?}, got {after:?}"
        );
        assert_eq!(
            stored_stamp(&engine, &tunnel_id, &label),
            before_stamp,
            "case {label:?}: a refused write must leave the stored lastUpdatedAt alone"
        );
    }
}
