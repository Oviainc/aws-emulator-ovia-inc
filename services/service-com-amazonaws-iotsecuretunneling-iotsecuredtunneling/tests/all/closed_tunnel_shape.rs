// SPDX-License-Identifier: CC0-1.0

//! The shape of a `CLOSED` tunnel record, however the tunnel stopped being
//! `OPEN`.
//!
//! RULE: a tunnel that stops being `OPEN` ends up in exactly one shape — its
//! status is `CLOSED`, neither end is connected to it ("we close the WebSocket
//! connections between the client and proxy server", `CloseTunnel`'s own
//! documentation), the record's `lastUpdatedAt` and both ends' `lastUpdatedAt`
//! all carry the one instant the close happened at, and `createdAt` is
//! untouched. What differs between the two writers of the transition is ONLY
//! that instant: an explicit `CloseTunnel` closes at the time of the call, while
//! a lifetime that ran out closes at the instant the transition came due, which
//! `maxLifetimeTimeoutMinutes` ("The maximum amount of time (in minutes) a
//! tunnel can remain open") makes owed whether or not anybody called.
//!
//! The table below spans both writers — an explicit close with the `delete`
//! member absent and with `delete=false`, and a lifetime running out at each
//! published bound of `maxLifetimeTimeoutMinutes` (`range(min: 1, max: 720)`,
//! from
//! `smithy_query(query="shape",shape="TimeoutConfig",model_path="services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json")`)
//! plus an interior value — and, for the owed close, the operation that first
//! touches the record and so settles it (`DescribeTunnel`, `ListTunnels`,
//! `CloseTunnel`). Every row must leave the same shape, so a closed tunnel's
//! state depends on the rule and not on how it closed.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-west-2";

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

/// Which writer of the transition closed the tunnel.
#[derive(Debug, Clone, Copy)]
enum ClosedBy {
    /// An explicit `CloseTunnel`; `delete_sent` is the `delete` member as sent,
    /// either absent or `false` — both keep the record readable.
    ExplicitClose { delete_sent: Option<bool> },
    /// The close the clock owes once a lifetime of `minutes` has run out,
    /// settled by the operation `noticed_by` names.
    LifetimeElapsed { minutes: i64, noticed_by: Noticer },
}

/// The operation that first touches a tunnel whose lifetime has run out, and so
/// settles the close the clock owes.
#[derive(Debug, Clone, Copy)]
enum Noticer {
    DescribeTunnel,
    ListTunnels,
    CloseTunnel,
}

/// The shape of a closed record, with the instant stamped deliberately left
/// out: this is what every way of closing must agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClosedShape {
    status: Option<String>,
    source_status: Option<String>,
    destination_status: Option<String>,
    /// Whether the record's stamp and both ends' stamps are the one instant.
    one_stored_instant: bool,
    /// Whether the three stamps a client reads back are that same one instant.
    one_reported_instant: bool,
    /// Whether `createdAt` came through the transition untouched.
    created_at_preserved: bool,
    /// Whether the close is stamped no earlier than the creation it follows.
    stamped_at_or_after_created_at: bool,
}

/// The shape every row of the table must produce.
fn expected_shape() -> ClosedShape {
    ClosedShape {
        status: Some("CLOSED".to_string()),
        source_status: Some("DISCONNECTED".to_string()),
        destination_status: Some("DISCONNECTED".to_string()),
        one_stored_instant: true,
        one_reported_instant: true,
        created_at_preserved: true,
        stamped_at_or_after_created_at: true,
    }
}

/// What `DescribeTunnel` reports about one tunnel.
#[derive(Debug, Clone)]
struct Described {
    status: Option<String>,
    created_at: Option<f64>,
    last_updated_at: Option<f64>,
    source_status: Option<String>,
    source_last_updated_at: Option<f64>,
    destination_status: Option<String>,
    destination_last_updated_at: Option<f64>,
}

/// Read `tunnel_id` back through `DescribeTunnel`, or panic naming `case`.
async fn describe(router: &MultiServiceRouter, tunnel_id: &str, case: &str) -> Described {
    let (status, body) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a closed tunnel stays readable, DescribeTunnel answered {status} {body:?}"
    );
    let tunnel = &body["tunnel"];
    Described {
        status: tunnel["status"].as_str().map(str::to_string),
        created_at: tunnel["createdAt"].as_f64(),
        last_updated_at: tunnel["lastUpdatedAt"].as_f64(),
        source_status: tunnel["sourceConnectionState"]["status"]
            .as_str()
            .map(str::to_string),
        source_last_updated_at: tunnel["sourceConnectionState"]["lastUpdatedAt"].as_f64(),
        destination_status: tunnel["destinationConnectionState"]["status"]
            .as_str()
            .map(str::to_string),
        destination_last_updated_at: tunnel["destinationConnectionState"]["lastUpdatedAt"].as_f64(),
    }
}

/// The `(status, lastUpdatedAt)` `ListTunnels` reports for `tunnel_id`.
async fn listed(router: &MultiServiceRouter, tunnel_id: &str) -> (Option<String>, Option<f64>) {
    let (_, body) = call(router, "ListTunnels", &json!({})).await;
    let summary = body["tunnelSummaries"]
        .as_array()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| summary["tunnelId"].as_str() == Some(tunnel_id))
                .cloned()
        })
        .unwrap_or(Value::Null);
    (
        summary["status"].as_str().map(str::to_string),
        summary["lastUpdatedAt"].as_f64(),
    )
}

/// The stored stamps of `tunnel_id`: `(record, source end, destination end)`.
fn stored_stamps(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
) -> (SystemTime, SystemTime, SystemTime) {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| {
            (
                tunnel.last_updated_at,
                tunnel.source_connection_state.last_updated_at,
                tunnel.destination_connection_state.last_updated_at,
            )
        })
        .unwrap_or_else(|| panic!("tunnel {tunnel_id} is not in the store"))
}

/// The stored `createdAt` of `tunnel_id`, which no close may touch.
fn stored_created_at(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str) -> SystemTime {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| tunnel.created_at)
        .unwrap_or_else(|| panic!("tunnel {tunnel_id} is not in the store"))
}

/// Open one tunnel and return its `tunnelId`. `minutes` is the
/// `maxLifetimeTimeoutMinutes` to configure, or `None` for the modelled default.
async fn open_tunnel(router: &MultiServiceRouter, case: &str, minutes: Option<i64>) -> String {
    let mut input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-closed-shape", "services": ["SSH"]},
    });
    if let Some(minutes) = minutes {
        input["timeoutConfig"] = json!({"maxLifetimeTimeoutMinutes": minutes});
    }
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(
        status, 200,
        "case {case:?}: OpenTunnel answered {status} {opened:?}"
    );
    opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
        .to_string()
}

/// Move the whole timeline of `tunnel_id` back by `shift`, so every stamp the
/// record carries is in the past and a stamp written later is plainly visible
/// rather than hidden by clock resolution. Returns the rewound `expires_at`.
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

/// One closed tunnel of the table.
struct Closed {
    tunnel_id: String,
    /// The `createdAt` the record carried before it closed.
    created_at_before: SystemTime,
    /// The instant the close was owed at, when the clock owed it; `None` for an
    /// explicit close, which is stamped with the time of the call.
    due_at: Option<SystemTime>,
}

/// Bring a fresh tunnel to `CLOSED` the way `how` says.
async fn closed_tunnel(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    how: ClosedBy,
    case: &str,
) -> Closed {
    match how {
        ClosedBy::ExplicitClose { delete_sent } => {
            let tunnel_id = open_tunnel(router, case, None).await;
            // Everything the open wrote is now two minutes old, so the close's
            // own stamp is strictly later whatever the clock's resolution.
            rewind(engine, &tunnel_id, Duration::from_secs(120), case);
            let created_at_before = stored_created_at(engine, &tunnel_id);
            let mut input = json!({"tunnelId": &tunnel_id});
            if let Some(delete) = delete_sent {
                input["delete"] = Value::Bool(delete);
            }
            let (status, body) = call(router, "CloseTunnel", &input).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CloseTunnel answered {status} {body:?}"
            );
            Closed {
                tunnel_id,
                created_at_before,
                due_at: None,
            }
        }
        ClosedBy::LifetimeElapsed {
            minutes,
            noticed_by,
        } => {
            let tunnel_id = open_tunnel(router, case, Some(minutes)).await;
            // Its lifetime ran out a second ago, exactly as the clock would have
            // it.
            let minutes = u64::try_from(minutes).unwrap_or_else(|_| {
                panic!("case {case:?}: maxLifetimeTimeoutMinutes is bounded at range(min: 1)")
            });
            let shift = Duration::from_secs(minutes * 60 + 1);
            let expires_at = rewind(engine, &tunnel_id, shift, case);
            let created_at_before = stored_created_at(engine, &tunnel_id);
            // The operation that touches the record settles the owed close.
            let (status, body) = match noticed_by {
                Noticer::DescribeTunnel => {
                    call(router, "DescribeTunnel", &json!({"tunnelId": &tunnel_id})).await
                }
                Noticer::ListTunnels => call(router, "ListTunnels", &json!({})).await,
                Noticer::CloseTunnel => {
                    call(router, "CloseTunnel", &json!({"tunnelId": &tunnel_id})).await
                }
            };
            assert_eq!(
                status, 200,
                "case {case:?}: the call that notices the owed close answered {status} {body:?}"
            );
            Closed {
                tunnel_id,
                created_at_before,
                due_at: Some(expires_at),
            }
        }
    }
}

#[tokio::test]
async fn every_way_of_closing_a_tunnel_leaves_the_same_closed_record() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let rows = [
        ClosedBy::ExplicitClose { delete_sent: None },
        ClosedBy::ExplicitClose {
            delete_sent: Some(false),
        },
        ClosedBy::LifetimeElapsed {
            minutes: 1,
            noticed_by: Noticer::DescribeTunnel,
        },
        ClosedBy::LifetimeElapsed {
            minutes: 60,
            noticed_by: Noticer::ListTunnels,
        },
        ClosedBy::LifetimeElapsed {
            minutes: 720,
            noticed_by: Noticer::CloseTunnel,
        },
        ClosedBy::LifetimeElapsed {
            minutes: 720,
            noticed_by: Noticer::DescribeTunnel,
        },
    ];

    // One rule, one shape: every row is compared against the SAME expected
    // shape, so a row that differed from any other row could not pass.
    for how in rows {
        let case = format!("{how:?}");
        let closed = closed_tunnel(&router, &engine, how, &case).await;
        let tunnel_id = closed.tunnel_id.as_str();

        let described = describe(&router, tunnel_id, &case).await;
        let (record, source, destination) = stored_stamps(&engine, tunnel_id);

        // The instant is the only thing a way of closing chooses: the owed close
        // stamps the instant it came due, an explicit close the time of the call.
        if let Some(due_at) = closed.due_at {
            assert_eq!(
                (record, source, destination),
                (due_at, due_at, due_at),
                "case {case:?}: the owed close stamps the instant it came due {due_at:?}, \
                 got {:?}",
                (record, source, destination)
            );
        } else {
            assert!(
                record > closed.created_at_before,
                "case {case:?}: an explicit close stamps the time of the call, expected a \
                 stamp after createdAt {:?}, got {record:?}",
                closed.created_at_before
            );
        }

        // Both reporting operations agree about the record they share.
        let (list_status, list_last_updated_at) = listed(&router, tunnel_id).await;
        assert_eq!(
            (list_status.as_deref(), list_last_updated_at),
            (described.status.as_deref(), described.last_updated_at),
            "case {case:?}: ListTunnels must agree with DescribeTunnel, got \
             {list_status:?} {list_last_updated_at:?}"
        );

        let shape = ClosedShape {
            status: described.status.clone(),
            source_status: described.source_status.clone(),
            destination_status: described.destination_status.clone(),
            one_stored_instant: record == source && record == destination,
            one_reported_instant: described.last_updated_at.is_some()
                && described.last_updated_at == described.source_last_updated_at
                && described.last_updated_at == described.destination_last_updated_at,
            created_at_preserved: stored_created_at(&engine, tunnel_id)
                == closed.created_at_before
                && described.created_at.is_some(),
            stamped_at_or_after_created_at: record >= closed.created_at_before,
        };
        assert_eq!(
            shape,
            expected_shape(),
            "case {case:?}: the closed record must not depend on how the tunnel closed, \
             expected {:?}, got {shape:?}",
            expected_shape()
        );
    }
}
