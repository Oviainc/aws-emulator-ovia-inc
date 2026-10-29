// SPDX-License-Identifier: CC0-1.0

//! Settling the close the clock owes, whichever entry point the client used.
//!
//! RULE: a tunnel whose `maxLifetimeTimeoutMinutes` ("The maximum amount of time
//! (in minutes) a tunnel can remain open") has run out is owed the close by the
//! clock whether or not anybody called, so EVERY operation of this service —
//! reporting or mutating, addressed by `tunnelId`, by `resourceArn` or
//! addressing no existing tunnel at all — settles that owed transition before it
//! answers, and none of them leaves an expired record behind still stored as
//! `OPEN`. The transition is stamped with the instant it came due, not with the
//! time of the call that noticed it.
//!
//! The table below spans every operation this service serves, each row varying
//! the entry point, the `maxLifetimeTimeoutMinutes` the lifetime ran out from
//! (both published bounds of `range(min: 1, max: 720)` and an interior value,
//! from
//! `smithy_query(query="shape",shape="TimeoutConfig",model_path="services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json")`)
//! and whether the tunnel carries tags. The settled record is read straight out
//! of the store rather than through `DescribeTunnel`, so what a row observes is
//! what its own call left behind and not what the reading call would settle for
//! it.
//!
//! The second test pins the other half of the rule: an operation settles only
//! what the clock already owes, so a tunnel still inside its lifetime is left
//! `OPEN`.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    state::TunnelStatus, ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState,
    EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The tag every tagged row's tunnel carries.
const TAG_KEY: &str = "stage";
const TAG_VALUE: &str = "settle";

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

/// The operation a row drives, and so the entry point that must settle the owed
/// close. Every operation this service serves appears here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// Addressed by `resourceArn`.
    ListTagsForResource,
    TagResource,
    UntagResource,
    /// Addressed by `tunnelId`.
    DescribeTunnel,
    CloseTunnel,
    RotateTunnelAccessToken,
    /// Addresses no existing tunnel.
    ListTunnels,
    OpenTunnel,
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
struct Row {
    entry: Entry,
    /// The `maxLifetimeTimeoutMinutes` the tunnel was opened with.
    minutes: i64,
    /// Whether the tunnel carries a tag before the call.
    tagged: bool,
    /// The HTTP status the call itself answers: every entry point serves the
    /// request, except that no access token is minted for a tunnel that is no
    /// longer open.
    expected_status: u16,
}

/// What the store holds for one tunnel after the call under test.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Settled {
    /// The record's status, on the wire.
    status: &'static str,
    /// Whether the close is stamped with the instant it came due.
    closed_when_due: bool,
}

/// The settled state every row must leave behind.
fn expected_settled() -> Settled {
    Settled {
        status: "CLOSED",
        closed_when_due: true,
    }
}

/// Read `tunnel_id` straight out of the store, so the observation itself settles
/// nothing.
fn stored(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str, case: &str) -> Settled {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| Settled {
            status: tunnel.status.as_wire(),
            closed_when_due: tunnel.closed_at == Some(tunnel.expires_at),
        })
        .unwrap_or_else(|| panic!("case {case:?}: tunnel {tunnel_id} is not in the store"))
}

/// Open one tunnel with `minutes` of lifetime and return `(tunnelId, tunnelArn)`.
async fn open_tunnel(
    router: &MultiServiceRouter,
    case: &str,
    minutes: i64,
) -> (String, String) {
    let input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-owed-close", "services": ["SSH"]},
        "timeoutConfig": {"maxLifetimeTimeoutMinutes": minutes},
    });
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(
        status, 200,
        "case {case:?}: OpenTunnel answered {status} {opened:?}"
    );
    let tunnel_id = opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
        .to_string();
    let tunnel_arn = opened["tunnelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnelArn in {opened:?}"))
        .to_string();
    (tunnel_id, tunnel_arn)
}

/// Move the whole timeline of `tunnel_id` back by `shift`, so the lifetime has
/// run out exactly as the clock would have it.
fn rewind(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    shift: Duration,
    case: &str,
) -> SystemTime {
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let tunnel = store
        .tunnel_mut(REGION, tunnel_id)
        .unwrap_or_else(|| panic!("case {case:?}: the opened tunnel was not persisted"));
    tunnel.created_at -= shift;
    tunnel.last_updated_at -= shift;
    tunnel.expires_at -= shift;
    tunnel.source_connection_state.last_updated_at -= shift;
    tunnel.destination_connection_state.last_updated_at -= shift;
    let expires_at = tunnel.expires_at;
    // The store indexes the soonest expiry of any open tunnel, so a rewound
    // record's new expiry has to be noted for the sweep to see it.
    store.note_open_expiry(expires_at);
    expires_at
}

/// Drive the row's entry point once, against the tunnel it addresses.
async fn invoke(
    router: &MultiServiceRouter,
    entry: Entry,
    tunnel_id: &str,
    tunnel_arn: &str,
    case: &str,
) -> (u16, Value) {
    match entry {
        Entry::ListTagsForResource => {
            call(
                router,
                "ListTagsForResource",
                &json!({"resourceArn": tunnel_arn}),
            )
            .await
        }
        Entry::TagResource => {
            call(
                router,
                "TagResource",
                &json!({"resourceArn": tunnel_arn, "tags": [{"key": "owner", "value": "fixture"}]}),
            )
            .await
        }
        Entry::UntagResource => {
            call(
                router,
                "UntagResource",
                &json!({"resourceArn": tunnel_arn, "tagKeys": [TAG_KEY]}),
            )
            .await
        }
        Entry::DescribeTunnel => {
            call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await
        }
        Entry::CloseTunnel => call(router, "CloseTunnel", &json!({"tunnelId": tunnel_id})).await,
        Entry::RotateTunnelAccessToken => {
            call(
                router,
                "RotateTunnelAccessToken",
                &json!({"tunnelId": tunnel_id, "clientMode": "SOURCE"}),
            )
            .await
        }
        Entry::ListTunnels => call(router, "ListTunnels", &json!({})).await,
        // A tunnel of its own, so this row addresses the expired record through
        // nothing at all and still has to settle it.
        Entry::OpenTunnel => {
            // `open_tunnel` asserts the 200 itself and hands back the new
            // tunnel's identifiers, which this row does not address.
            let (opened_id, _) = open_tunnel(router, case, 720).await;
            (200, json!({"tunnelId": opened_id}))
        }
    }
}

#[tokio::test]
async fn every_entry_point_settles_the_owed_close_before_answering() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let rows = [
        Row {
            entry: Entry::ListTagsForResource,
            minutes: 1,
            tagged: true,
            expected_status: 200,
        },
        Row {
            entry: Entry::ListTagsForResource,
            minutes: 720,
            tagged: false,
            expected_status: 200,
        },
        Row {
            entry: Entry::ListTagsForResource,
            minutes: 60,
            tagged: true,
            expected_status: 200,
        },
        Row {
            entry: Entry::TagResource,
            minutes: 60,
            tagged: false,
            expected_status: 200,
        },
        Row {
            entry: Entry::UntagResource,
            minutes: 1,
            tagged: true,
            expected_status: 200,
        },
        Row {
            entry: Entry::DescribeTunnel,
            minutes: 720,
            tagged: false,
            expected_status: 200,
        },
        Row {
            entry: Entry::CloseTunnel,
            minutes: 1,
            tagged: false,
            expected_status: 200,
        },
        Row {
            // A closed tunnel is never connected to again, so the rotate is
            // refused — after the owed close has been settled all the same.
            entry: Entry::RotateTunnelAccessToken,
            minutes: 720,
            tagged: false,
            expected_status: 400,
        },
        Row {
            entry: Entry::ListTunnels,
            minutes: 60,
            tagged: true,
            expected_status: 200,
        },
        Row {
            entry: Entry::OpenTunnel,
            minutes: 1,
            tagged: false,
            expected_status: 200,
        },
    ];

    // One rule, one settled state: every row is compared against the SAME
    // expected state, so a row that differed from any other could not pass.
    for row in rows {
        let case = format!("{row:?}");
        let (tunnel_id, tunnel_arn) = open_tunnel(&router, &case, row.minutes).await;
        if row.tagged {
            let (status, body) = call(
                &router,
                "TagResource",
                &json!({
                    "resourceArn": &tunnel_arn,
                    "tags": [{"key": TAG_KEY, "value": TAG_VALUE}],
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: tagging the fixture answered {status} {body:?}"
            );
        }

        // Its lifetime ran out a second ago.
        let minutes = u64::try_from(row.minutes).unwrap_or_else(|_| {
            panic!("case {case:?}: maxLifetimeTimeoutMinutes is bounded at range(min: 1)")
        });
        rewind(
            &engine,
            &tunnel_id,
            Duration::from_secs(minutes * 60 + 1),
            &case,
        );

        let (status, body) = invoke(&router, row.entry, &tunnel_id, &tunnel_arn, &case).await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} {body:?}",
            row.expected_status
        );

        // The response the entry point serves is unchanged by the settle: the
        // tags are exactly what is stored for the resource.
        if row.entry == Entry::ListTagsForResource {
            let reported = body["tags"].as_array().map(|tags| {
                tags.iter()
                    .map(|tag| {
                        (
                            tag["key"].as_str().unwrap_or_default().to_string(),
                            tag["value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect::<Vec<_>>()
            });
            let expected = row
                .tagged
                .then(|| vec![(TAG_KEY.to_string(), TAG_VALUE.to_string())]);
            assert_eq!(
                reported, expected,
                "case {case:?}: ListTagsForResource must report the stored tags, \
                 expected {expected:?}, got {reported:?}"
            );
        }

        let settled = stored(&engine, &tunnel_id, &case);
        assert_eq!(
            settled,
            expected_settled(),
            "case {case:?}: the call must leave no expired tunnel stored as OPEN, \
             expected {:?}, got {settled:?}",
            expected_settled()
        );
    }
}

#[tokio::test]
async fn an_entry_point_settles_only_what_the_clock_already_owes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // A lifetime of 720 minutes (the published maximum) has plainly not run out,
    // so no entry point may close the tunnel early.
    for entry in [
        Entry::ListTagsForResource,
        Entry::DescribeTunnel,
        Entry::ListTunnels,
        Entry::TagResource,
    ] {
        let case = format!("{entry:?} on a live tunnel");
        let (tunnel_id, tunnel_arn) = open_tunnel(&router, &case, 720).await;
        let (status, body) = invoke(&router, entry, &tunnel_id, &tunnel_arn, &case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call answered {status} {body:?}"
        );

        let live = engine
            .state::<TunnelState>()
            .lock()
            .tunnel(REGION, &tunnel_id)
            .map(|tunnel| (tunnel.status, tunnel.closed_at))
            .unwrap_or_else(|| panic!("case {case:?}: tunnel {tunnel_id} is not in the store"));
        assert_eq!(
            live,
            (TunnelStatus::Open, None),
            "case {case:?}: a tunnel inside its lifetime stays OPEN, got {live:?}"
        );
    }
}
