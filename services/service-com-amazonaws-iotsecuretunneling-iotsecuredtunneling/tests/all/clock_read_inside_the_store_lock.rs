// SPDX-License-Identifier: CC0-1.0

//! The instant an operation reasons with is the instant at which it holds the
//! store.
//!
//! RULE: every operation of this service settles the clock-owed close and stamps
//! the records it changes with ONE instant, and that instant is read while the
//! store lock is held, not before it is acquired. Acquiring the lock blocks for
//! an unbounded time — every operation of this service takes the same one — so an
//! instant sampled before the acquisition describes a store the call has not seen
//! yet: a `maxLifetimeTimeoutMinutes` ("The maximum amount of time (in minutes) a
//! tunnel can remain open",
//! `smithy_query(query="shape",shape="TimeoutConfig",model_path="services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json")`)
//! that runs out while the call waits has run out by the time it reads or writes,
//! and a stamp it leaves can never predate the moment it held the store.
//!
//! Each test holds the store lock from another thread across the window the rule
//! is about, so the call under test is forced to wait through it. The three
//! tables span, in turn, every entry point the service serves (the owed close),
//! every entry point that writes a stamp (the stamp), and the published bounds of
//! the lifetime (the timeline of a tunnel opened after the wait).
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).
//!
//! This module owns the harness for driving one entry point while the store lock
//! is held from another thread — the router, the armed expiry and the per-entry
//! invocation — so the other suite about a call that waited for the store
//! (`tests/all/stamp_never_precedes_the_owed_close.rs`) drives its own rows
//! through this code rather than carrying a second copy of it.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    StoredTunnel, TunnelStatus,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "us-east-2";

/// How long before its expiry a tunnel is armed: the lock is held across this
/// window, so the call under test samples the clock before the expiry if it
/// samples before taking the lock, and after it if it samples while holding it.
const MARGIN: Duration = Duration::from_millis(150);

/// How long the lock is held from the other thread. Strictly longer than
/// [`MARGIN`], so the instant the holder releases the lock is always past the
/// armed expiry — whatever the machine's speed, which is why these tests need no
/// slack of their own.
pub(crate) const HOLD: Duration = Duration::from_millis(600);

/// The tag a row that removes one starts from.
pub(crate) const TAG_KEY: &str = "stage";
const TAG_VALUE: &str = "under-lock";

/// A router carrying only this service, over the engine the test also inspects.
pub(crate) fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    input: &Value,
) -> (u16, Value) {
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

/// Open one tunnel with `minutes` of lifetime and return `(tunnelId, tunnelArn)`.
pub(crate) async fn open_tunnel(
    router: &MultiServiceRouter,
    case: &str,
    minutes: i64,
) -> (String, String) {
    let input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-under-lock", "services": ["SSH"]},
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

/// The stored record for `tunnel_id`, read straight out of the store so the
/// observation itself settles nothing.
pub(crate) fn stored(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    case: &str,
) -> StoredTunnel {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: tunnel {tunnel_id} is not in the store"))
}

/// Move `tunnel_id`'s whole timeline so its lifetime runs out [`MARGIN`] from
/// now, and answer that expiry: the window the lock is then held across.
pub(crate) fn arm_expiry(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    case: &str,
) -> SystemTime {
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let tunnel = store
        .tunnel_mut(REGION, tunnel_id)
        .unwrap_or_else(|| panic!("case {case:?}: the opened tunnel was not persisted"));
    let expires_at = SystemTime::now() + MARGIN;
    let shift = tunnel
        .expires_at
        .duration_since(expires_at)
        .unwrap_or_else(|_| panic!("case {case:?}: the fixture's lifetime outlasts the margin"));
    tunnel.created_at -= shift;
    tunnel.last_updated_at -= shift;
    tunnel.source_connection_state.last_updated_at -= shift;
    tunnel.destination_connection_state.last_updated_at -= shift;
    tunnel.expires_at = expires_at;
    // The store indexes the soonest expiry of any open tunnel, so a rewound
    // record's new expiry has to be noted for the sweep to see it.
    store.note_open_expiry(expires_at);
    expires_at
}

/// The store lock, held from another thread until it has been held for
/// [`HOLD`]. `join` answers the instant it was released, which every call that
/// waited for it only reaches the store after.
struct LockHold {
    handle: JoinHandle<SystemTime>,
}

impl LockHold {
    /// Wait for the holder to let go and answer the instant it did.
    fn released_at(self, case: &str) -> SystemTime {
        self.handle
            .join()
            .unwrap_or_else(|_| panic!("case {case:?}: the lock-holding thread panicked"))
    }
}

/// Take the store lock from another thread and keep it for [`HOLD`].
///
/// Returns once the lock is actually held, so the request issued next is
/// guaranteed to wait for it rather than racing it.
fn hold_store_lock(engine: &Arc<engine::InMemoryEngine>, case: &str) -> LockHold {
    let state = engine.state::<TunnelState>();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let guard = state.lock();
        acquired_tx
            .send(())
            .expect("report that the store lock is held");
        std::thread::sleep(HOLD);
        // Sampled before the guard is dropped, so no other thread can have
        // touched the store at or before this instant.
        let released_at = SystemTime::now();
        drop(guard);
        released_at
    });
    acquired_rx
        .recv()
        .unwrap_or_else(|_| panic!("case {case:?}: the store lock was never taken"));
    LockHold { handle }
}

/// The operation a row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
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

/// Drive `entry` once against the tunnel it addresses.
pub(crate) async fn invoke(
    router: &MultiServiceRouter,
    entry: Entry,
    client_mode: &str,
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
                &json!({"tunnelId": tunnel_id, "clientMode": client_mode}),
            )
            .await
        }
        Entry::ListTunnels => call(router, "ListTunnels", &json!({})).await,
        // A tunnel of its own, so this row addresses the armed record through
        // nothing at all and still has to settle it.
        Entry::OpenTunnel => {
            let (opened_id, _) = open_tunnel(router, case, 720).await;
            (200, json!({"tunnelId": opened_id}))
        }
    }
}

/// Give `tunnel_arn` the tag a row that removes one starts from.
pub(crate) async fn tag_fixture(router: &MultiServiceRouter, tunnel_arn: &str, case: &str) {
    let (status, body) = call(
        router,
        "TagResource",
        &json!({
            "resourceArn": tunnel_arn,
            "tags": [{"key": TAG_KEY, "value": TAG_VALUE}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: tagging the fixture answered {status} {body:?}"
    );
}

/// One row of the owed-close table.
#[derive(Debug, Clone, Copy)]
struct SettleRow {
    entry: Entry,
    /// The `clientMode` a rotating row asks for.
    client_mode: &'static str,
    /// Whether the tunnel carries a tag before the call.
    tagged: bool,
    /// The HTTP status the call itself answers: every entry point serves the
    /// request, except that no access token is minted for a tunnel that is no
    /// longer open.
    expected_status: u16,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expiry_inside_the_lock_wait_is_settled_by_the_call_that_waited() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let rows = [
        SettleRow {
            entry: Entry::ListTagsForResource,
            client_mode: "ALL",
            tagged: true,
            expected_status: 200,
        },
        SettleRow {
            entry: Entry::TagResource,
            client_mode: "ALL",
            tagged: false,
            expected_status: 200,
        },
        SettleRow {
            entry: Entry::UntagResource,
            client_mode: "ALL",
            tagged: true,
            expected_status: 200,
        },
        SettleRow {
            entry: Entry::DescribeTunnel,
            client_mode: "ALL",
            tagged: false,
            expected_status: 200,
        },
        SettleRow {
            entry: Entry::CloseTunnel,
            client_mode: "ALL",
            tagged: true,
            expected_status: 200,
        },
        SettleRow {
            // A tunnel whose lifetime ran out while this call waited for the
            // store is no longer connectable, so the rotate is refused instead
            // of minting tokens for it.
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "ALL",
            tagged: false,
            expected_status: 400,
        },
        SettleRow {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "SOURCE",
            tagged: true,
            expected_status: 400,
        },
        SettleRow {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "DESTINATION",
            tagged: false,
            expected_status: 400,
        },
        SettleRow {
            entry: Entry::ListTunnels,
            client_mode: "ALL",
            tagged: true,
            expected_status: 200,
        },
        SettleRow {
            entry: Entry::OpenTunnel,
            client_mode: "ALL",
            tagged: false,
            expected_status: 200,
        },
    ];

    for row in rows {
        let case = format!("{row:?}");
        let (tunnel_id, tunnel_arn) = open_tunnel(&router, &case, 720).await;
        if row.tagged {
            tag_fixture(&router, &tunnel_arn, &case).await;
        }
        let before = stored(&engine, &tunnel_id, &case);
        assert_eq!(
            (before.status, before.closed_at),
            (TunnelStatus::Open, None),
            "case {case:?}: the fixture starts OPEN, got {:?}",
            (before.status, before.closed_at)
        );

        // Its lifetime runs out while the call below is waiting for the store.
        let expires_at = arm_expiry(&engine, &tunnel_id, &case);
        let hold = hold_store_lock(&engine, &case);
        let (status, body) = invoke(
            &router,
            row.entry,
            row.client_mode,
            &tunnel_id,
            &tunnel_arn,
            &case,
        )
        .await;
        let released_at = hold.released_at(&case);
        assert!(
            released_at > expires_at,
            "case {case:?}: the lock must be held past the armed expiry, \
             released at {released_at:?}, expiry {expires_at:?}"
        );
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} {body:?}",
            row.expected_status
        );

        // The refusal a rotate answers for a tunnel that is no longer open.
        if row.entry == Entry::RotateTunnelAccessToken {
            let code = body["__type"].as_str().unwrap_or_default();
            let message = body["message"].as_str().unwrap_or_default();
            let expected_message = format!("Tunnel {tunnel_id} is close.");
            assert_eq!(
                (code, message),
                ("ValidationException", expected_message.as_str()),
                "case {case:?}: expected ValidationException {expected_message:?}, \
                 got {code:?} {message:?}"
            );
            assert!(
                body["sourceAccessToken"].is_null() && body["destinationAccessToken"].is_null(),
                "case {case:?}: a refused rotate mints no token, got {body:?}"
            );
            let after = stored(&engine, &tunnel_id, &case);
            assert_eq!(
                (
                    after.source_access_token.as_str(),
                    after.destination_access_token.as_str()
                ),
                (
                    before.source_access_token.as_str(),
                    before.destination_access_token.as_str()
                ),
                "case {case:?}: a refused rotate leaves the stored tokens as they were"
            );
        }

        // Whatever the entry point answered, the expired record is settled: it is
        // stored CLOSED, stamped with the instant the close came due and not with
        // the time of the call that noticed it.
        let settled = stored(&engine, &tunnel_id, &case);
        assert_eq!(
            (settled.status, settled.closed_at),
            (TunnelStatus::Closed, Some(expires_at)),
            "case {case:?}: the call must leave no expired tunnel stored as OPEN, \
             expected CLOSED at {expires_at:?}, got {:?}",
            (settled.status, settled.closed_at)
        );
    }
}

/// One row of the stamp table: an entry point that changes something the tunnel
/// reports, and so stamps `Tunnel$lastUpdatedAt` ("The last time the tunnel was
/// updated").
#[derive(Debug, Clone, Copy)]
struct StampRow {
    entry: Entry,
    client_mode: &'static str,
    tagged: bool,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stamp_is_never_earlier_than_the_moment_the_call_held_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let rows = [
        StampRow {
            entry: Entry::TagResource,
            client_mode: "ALL",
            tagged: false,
        },
        StampRow {
            entry: Entry::UntagResource,
            client_mode: "ALL",
            tagged: true,
        },
        StampRow {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "ALL",
            tagged: false,
        },
        StampRow {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "SOURCE",
            tagged: true,
        },
        StampRow {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "DESTINATION",
            tagged: false,
        },
        StampRow {
            entry: Entry::CloseTunnel,
            client_mode: "ALL",
            tagged: true,
        },
    ];

    for row in rows {
        let case = format!("{row:?}");
        // The published maximum lifetime, so nothing is owed a close here: this
        // half of the rule is about the stamp alone.
        let (tunnel_id, tunnel_arn) = open_tunnel(&router, &case, 720).await;
        if row.tagged {
            tag_fixture(&router, &tunnel_arn, &case).await;
        }
        let before = stored(&engine, &tunnel_id, &case);

        let hold = hold_store_lock(&engine, &case);
        let (status, body) = invoke(
            &router,
            row.entry,
            row.client_mode,
            &tunnel_id,
            &tunnel_arn,
            &case,
        )
        .await;
        let released_at = hold.released_at(&case);
        assert_eq!(
            status, 200,
            "case {case:?}: the call answered {status} {body:?}"
        );

        let after = stored(&engine, &tunnel_id, &case);
        assert!(
            after.last_updated_at >= released_at,
            "case {case:?}: lastUpdatedAt must be the instant the call held the store, \
             expected at least {released_at:?}, got {:?}",
            after.last_updated_at
        );
        assert!(
            after.last_updated_at > before.last_updated_at,
            "case {case:?}: an update moves lastUpdatedAt forward, \
             expected later than {:?}, got {:?}",
            before.last_updated_at,
            after.last_updated_at
        );
        // The expiry is a property of the open, which a later call neither
        // extends nor shortens.
        assert_eq!(
            after.expires_at, before.expires_at,
            "case {case:?}: the lifetime stays the one the open computed, expected {:?}, got {:?}",
            before.expires_at, after.expires_at
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_opened_tunnels_timeline_starts_when_the_call_held_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // `TimeoutConfig$maxLifetimeTimeoutMinutes` is `range(min: 1, max: 720)`
    // (`smithy_query(query="shape",shape="TimeoutConfig")`): both published
    // bounds and an interior value.
    for minutes in [1i64, 60, 720] {
        let case = format!("OpenTunnel with maxLifetimeTimeoutMinutes {minutes}");
        let hold = hold_store_lock(&engine, &case);
        let (tunnel_id, _) = open_tunnel(&router, &case, minutes).await;
        let released_at = hold.released_at(&case);

        let opened = stored(&engine, &tunnel_id, &case);
        assert!(
            opened.created_at >= released_at,
            "case {case:?}: createdAt must be the instant the open held the store, \
             expected at least {released_at:?}, got {:?}",
            opened.created_at
        );
        let lifetime = Duration::from_secs(
            u64::try_from(minutes).unwrap_or_else(|_| {
                panic!("case {case:?}: maxLifetimeTimeoutMinutes is bounded at range(min: 1)")
            }) * 60,
        );
        assert_eq!(
            opened.expires_at,
            opened.created_at + lifetime,
            "case {case:?}: the lifetime runs from the instant the tunnel was created, \
             expected {:?}, got {:?}",
            opened.created_at + lifetime,
            opened.expires_at
        );
    }
}
