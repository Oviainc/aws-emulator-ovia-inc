// SPDX-License-Identifier: CC0-1.0

//! `CloseTunnel` applied to a tunnel that is no longer `OPEN`.
//!
//! RULE: a call that applies a state transition applies it only while the
//! resolved record is still in the state the transition leaves, so a call that
//! finds the resource already in the target state changes nothing about it —
//! neither its status, nor the connection state of either end, nor any
//! `lastUpdatedAt` stamp. The stamps a closed tunnel carries are the stamps of
//! the close that actually happened: for a lifetime that ran out that is the
//! instant the close came DUE, which is what
//! `maxLifetimeTimeoutMinutes` ("the maximum amount of time (in minutes) a
//! tunnel can remain open") makes owed whether or not anybody called.
//!
//! The table below spans how a tunnel reaches `CLOSED` (an explicit close with
//! the `delete` member absent and with `delete=false`, and a lifetime running
//! out at each published bound of `maxLifetimeTimeoutMinutes` plus an interior
//! value), the form of the repeated close, and how many times it is repeated.
//! `delete=true` is a different request — it removes the tunnel's data — so it
//! is asserted on its own below: the idempotence of the transition must not stop
//! an already-closed record from being deleted.
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
const REGION: &str = "eu-west-1";

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

/// Everything a `CloseTunnel` on an already-closed tunnel must leave alone, as
/// a client reads it back through `DescribeTunnel`.
#[derive(Debug, Clone, PartialEq)]
struct Reported {
    status: Option<String>,
    created_at: Option<f64>,
    last_updated_at: Option<f64>,
    source_status: Option<String>,
    source_last_updated_at: Option<f64>,
    destination_status: Option<String>,
    destination_last_updated_at: Option<f64>,
}

/// `DescribeTunnel`'s report of `tunnel_id`, or a panic naming `case`.
async fn reported(router: &MultiServiceRouter, tunnel_id: &str, case: &str) -> Reported {
    let (status, described) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a closed tunnel stays readable, DescribeTunnel answered {described:?}"
    );
    let tunnel = &described["tunnel"];
    Reported {
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

/// The `lastUpdatedAt` `ListTunnels` reports for `tunnel_id`, so the two readers
/// of the same record can be compared.
async fn listed_last_updated_at(router: &MultiServiceRouter, tunnel_id: &str) -> Option<f64> {
    let (_, listed) = call(router, "ListTunnels", &json!({})).await;
    listed["tunnelSummaries"]
        .as_array()?
        .iter()
        .find(|summary| summary["tunnelId"].as_str() == Some(tunnel_id))
        .and_then(|summary| summary["lastUpdatedAt"].as_f64())
}

/// The stored stamps of `tunnel_id`: `(lastUpdatedAt, source, destination)`.
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

/// Open one tunnel and return its `tunnelId`. `minutes` is the
/// `maxLifetimeTimeoutMinutes` to configure, or `None` for the modelled default.
async fn open_tunnel(router: &MultiServiceRouter, case: &str, minutes: Option<i64>) -> String {
    let mut input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-idempotent", "services": ["SSH"]},
    });
    if let Some(minutes) = minutes {
        input["timeoutConfig"] = json!({"maxLifetimeTimeoutMinutes": minutes});
    }
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(status, 200, "case {case:?}: OpenTunnel answered {opened:?}");
    opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
        .to_string()
}

/// Move the whole timeline of `tunnel_id` back by `shift`, so every stamp the
/// record carries is in the past: a call that re-stamped a field with its own
/// `now` would then be plainly visible rather than hidden by clock resolution.
fn rewind(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str, shift: Duration, case: &str) {
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
}

/// How a tunnel stops being `OPEN` while its record stays readable.
#[derive(Debug, Clone, Copy)]
enum ReachedClosed {
    /// An explicit `CloseTunnel`; the `delete` member as sent, which here is
    /// either absent or `false` — both keep the data.
    ExplicitClose { delete_sent: Option<bool> },
    /// The close the clock owes once a lifetime of `minutes` has run out.
    LifetimeElapsed { minutes: i64 },
}

/// A `CloseTunnel` request body naming `tunnel_id`, with the `delete` member as
/// `delete_sent` says.
fn close_request(tunnel_id: &str, delete_sent: Option<bool>) -> Value {
    let mut input = json!({"tunnelId": tunnel_id});
    if let Some(delete) = delete_sent {
        input["delete"] = Value::Bool(delete);
    }
    input
}

/// Bring a fresh tunnel to `CLOSED` the way `how` says and return its id,
/// alongside the instant the close happened at when the clock owed it.
async fn closed_tunnel(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    how: ReachedClosed,
    case: &str,
) -> (String, Option<SystemTime>) {
    match how {
        ReachedClosed::ExplicitClose { delete_sent } => {
            let tunnel_id = open_tunnel(router, case, None).await;
            let (status, closed) =
                call(router, "CloseTunnel", &close_request(&tunnel_id, delete_sent)).await;
            assert_eq!(status, 200, "case {case:?}: CloseTunnel answered {closed:?}");
            // Put the stamps the close just wrote into the past, so a later
            // re-stamp with the time of the noticing call cannot be mistaken for
            // them.
            rewind(engine, &tunnel_id, Duration::from_secs(120), case);
            (tunnel_id, None)
        }
        ReachedClosed::LifetimeElapsed { minutes } => {
            let tunnel_id = open_tunnel(router, case, Some(minutes)).await;
            // Its lifetime ran out a second ago, exactly as the clock would have
            // it; the store's "soonest expiry" index follows the rewound record.
            let shift = Duration::from_secs(u64::try_from(minutes).unwrap_or(1) * 60 + 1);
            rewind(engine, &tunnel_id, shift, case);
            let expires_at = {
                let state = engine.state::<TunnelState>();
                let mut store = state.lock();
                let expires_at = store
                    .tunnel(REGION, &tunnel_id)
                    .map(|tunnel| tunnel.expires_at)
                    .unwrap_or_else(|| panic!("case {case:?}: the tunnel left the store"));
                store.note_open_expiry(expires_at);
                expires_at
            };
            // The sweep settles the owed close; it stamps the instant it came due.
            let before = reported(router, &tunnel_id, case).await;
            assert_eq!(
                before.status.as_deref(),
                Some("CLOSED"),
                "case {case:?}: a tunnel past its lifetime is CLOSED, got {before:?}"
            );
            (tunnel_id, Some(expires_at))
        }
    }
}

#[tokio::test]
async fn closing_an_already_closed_tunnel_changes_nothing_it_reports() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let reached = [
        ReachedClosed::ExplicitClose { delete_sent: None },
        ReachedClosed::ExplicitClose {
            delete_sent: Some(false),
        },
        // Each published bound of `maxLifetimeTimeoutMinutes`
        // (`range(min: 1, max: 720)`) plus an interior value.
        ReachedClosed::LifetimeElapsed { minutes: 1 },
        ReachedClosed::LifetimeElapsed { minutes: 60 },
        ReachedClosed::LifetimeElapsed { minutes: 720 },
    ];

    for how in reached {
        for repeat_delete in [None, Some(false)] {
            for repeats in [1_usize, 3] {
                let case =
                    format!("{how:?}, then {repeats} close(s) with delete {repeat_delete:?}");
                let (tunnel_id, due_at) = closed_tunnel(&router, &engine, how, &case).await;

                let before = reported(&router, &tunnel_id, &case).await;
                let before_stored = stored_stamps(&engine, &tunnel_id);
                if let Some(due_at) = due_at {
                    assert_eq!(
                        before_stored,
                        (due_at, due_at, due_at),
                        "case {case:?}: the owed close stamps the instant it came due, \
                         got {before_stored:?}"
                    );
                }

                for attempt in 1..=repeats {
                    let (status, body) = call(
                        &router,
                        "CloseTunnel",
                        &close_request(&tunnel_id, repeat_delete),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: close #{attempt} answered {status} {body:?}"
                    );
                }

                let after = reported(&router, &tunnel_id, &case).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a close that finds the tunnel already CLOSED must change \
                     nothing, expected {before:?}, got {after:?}"
                );
                let after_stored = stored_stamps(&engine, &tunnel_id);
                assert_eq!(
                    after_stored, before_stored,
                    "case {case:?}: expected the stored stamps {before_stored:?}, \
                     got {after_stored:?}"
                );
                assert_eq!(
                    listed_last_updated_at(&router, &tunnel_id).await,
                    after.last_updated_at,
                    "case {case:?}: ListTunnels must report the same lastUpdatedAt as \
                     DescribeTunnel, got {after:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_first_close_of_an_open_tunnel_still_applies_the_transition() {
    // The control for the table above: the guard that makes a repeated close
    // idempotent must not stop the close that actually has work to do.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for delete_sent in [None, Some(false)] {
        let case = format!("the first close with delete {delete_sent:?}");
        let tunnel_id = open_tunnel(&router, &case, Some(720)).await;
        // Rewind so the stamps the close writes are strictly later than the ones
        // `OpenTunnel` wrote, whatever the clock's resolution.
        rewind(&engine, &tunnel_id, Duration::from_secs(120), &case);
        let before = reported(&router, &tunnel_id, &case).await;
        assert_eq!(
            before.status.as_deref(),
            Some("OPEN"),
            "case {case:?}: a fresh tunnel inside its lifetime is OPEN, got {before:?}"
        );

        let (status, closed) =
            call(&router, "CloseTunnel", &close_request(&tunnel_id, delete_sent)).await;
        assert_eq!(status, 200, "case {case:?}: CloseTunnel answered {closed:?}");

        let after = reported(&router, &tunnel_id, &case).await;
        assert_eq!(
            after.status.as_deref(),
            Some("CLOSED"),
            "case {case:?}: the close must move the status, got {after:?}"
        );
        assert_eq!(
            (after.source_status.as_deref(), after.destination_status.as_deref()),
            (Some("DISCONNECTED"), Some("DISCONNECTED")),
            "case {case:?}: neither end is connected to a closed tunnel, got {after:?}"
        );
        assert!(
            after.last_updated_at > before.last_updated_at,
            "case {case:?}: the close must stamp lastUpdatedAt, expected a stamp after \
             {:?}, got {:?}",
            before.last_updated_at,
            after.last_updated_at
        );
        let (stored, source, destination) = stored_stamps(&engine, &tunnel_id);
        assert_eq!(
            (source, destination),
            (stored, stored),
            "case {case:?}: one close stamps the record and both ends with one instant, \
             got {:?}",
            (stored, source, destination)
        );
        assert_eq!(
            after.created_at, before.created_at,
            "case {case:?}: a close leaves createdAt alone, got {after:?}"
        );
    }
}

#[tokio::test]
async fn delete_still_removes_a_tunnel_that_is_already_closed() {
    // `delete` is not the transition: whether the record is already CLOSED — by
    // an earlier call or by its lifetime running out — the call that asks for the
    // data to be deleted deletes it.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let reached = [
        ReachedClosed::ExplicitClose { delete_sent: None },
        ReachedClosed::ExplicitClose {
            delete_sent: Some(false),
        },
        ReachedClosed::LifetimeElapsed { minutes: 1 },
        ReachedClosed::LifetimeElapsed { minutes: 720 },
    ];

    for how in reached {
        let case = format!("{how:?}, then close with delete=true");
        let (tunnel_id, _) = closed_tunnel(&router, &engine, how, &case).await;

        let (status, deleted) = call(
            &router,
            "CloseTunnel",
            &close_request(&tunnel_id, Some(true)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CloseTunnel(delete) answered {status} {deleted:?}"
        );

        let (status, described) =
            call(&router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted tunnel must be gone, answered {described:?}"
        );
        assert!(
            engine
                .state::<TunnelState>()
                .lock()
                .tunnel(REGION, &tunnel_id)
                .is_none(),
            "case {case:?}: the record must leave the store"
        );
    }
}
