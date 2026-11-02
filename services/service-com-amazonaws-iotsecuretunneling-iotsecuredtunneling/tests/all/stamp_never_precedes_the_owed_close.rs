// SPDX-License-Identifier: CC0-1.0

//! The stamp a call leaves on a tunnel whose close the clock already owed.
//!
//! RULE: a tunnel's `lastUpdatedAt` ("The last time the tunnel was updated", the
//! modelled `Tunnel$lastUpdatedAt` documentation) never moves BACKWARDS. The
//! close owed a tunnel whose `maxLifetimeTimeoutMinutes` ("The maximum amount of
//! time (in minutes) a tunnel can remain open",
//! `smithy_query(query="shape",shape="TimeoutConfig",model_path="services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json")`)
//! has run out is stamped with the instant it came DUE, and every stamp a call
//! writes afterwards is the instant at which THAT call held the store — which is
//! at or after the due instant, whichever caller settled the owed close. A call
//! that changes nothing the tunnel reports writes no stamp at all, so the record
//! keeps the stamp its close left.
//!
//! The table spans what the rule generalises over: the entry point (every
//! operation the service serves, addressed by `tunnelId`, by `resourceArn` or
//! addressing no existing tunnel at all), whether the call changes something the
//! tunnel reports and so stamps it, and the lifetime the expiry came from (both
//! published bounds of `range(min: 1, max: 720)` and an interior value). Each of
//! the two tests then runs the whole table with the owed close settled by a
//! different caller: by the call under test itself, and by another caller that
//! held the store across the due instant and settled the close there — the case
//! in which the call under test finds the record already CLOSED and stamped, and
//! must not write an earlier instant over it.
//!
//! Every row forces the call under test to wait for the store across the due
//! instant, by holding the lock from another thread, so the window between
//! taking the lock and reasoning about the store is the window the rule is
//! about. The harness for that is [`super::clock_read_inside_the_store_lock`].
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::TunnelStatus;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState;
use smithy_server_runtime::MultiServiceRouter;

use super::clock_read_inside_the_store_lock::{
    arm_expiry, call, invoke, mount, open_tunnel, stored, tag_fixture, Entry, HOLD,
};

/// How far apart two epoch-second timestamps may be and still count as the same
/// instant: the wire carries a timestamp as an epoch-seconds number, whose
/// double rounds a current instant to well under a microsecond, and this slack
/// absorbs that rounding alone.
const TIMESTAMP_TOLERANCE: f64 = 1e-6;

/// Which caller settles the close the clock owes the armed tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettledBy {
    /// Nobody touches the store while the lock is held away from it, so the call
    /// under test is the one whose own settle step closes the expired record.
    TheCallThatWaited,
    /// The caller holding the store settles the owed close before it lets go, so
    /// the call under test reaches a record that is already CLOSED and already
    /// stamped with the due instant.
    AnotherCallerThatHeldTheStore,
}

/// `at` as the epoch-seconds number the protocol reports a timestamp as.
fn epoch_secs(at: SystemTime, case: &str) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| panic!("case {case:?}: {at:?} precedes the epoch"))
        .as_secs_f64()
}

/// Take the store lock from another thread, keep it for [`HOLD`] — past the
/// armed expiry — and, when `settled_by` asks for it, settle the owed close
/// under that same guard before letting go.
///
/// Returns once the lock is actually held, so the request issued next is
/// guaranteed to wait for it rather than racing it. Joining the handle answers
/// the instant the guard was dropped, which every call that waited for it only
/// reaches the store after.
fn hold_store_lock(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    settled_by: SettledBy,
) -> JoinHandle<SystemTime> {
    let state = engine.state::<TunnelState>();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let mut guard = state.lock();
        acquired_tx
            .send(())
            .expect("report that the store lock is held");
        std::thread::sleep(HOLD);
        if settled_by == SettledBy::AnotherCallerThatHeldTheStore {
            // The same sweep every entry point runs, here run by the caller that
            // holds the store: the owed close is settled, and final, before the
            // waiting call ever sees the record.
            guard.materialize_expired_tunnels(SystemTime::now());
        }
        // Sampled before the guard is dropped, so no other thread can have
        // touched the store at or before this instant.
        let released_at = SystemTime::now();
        drop(guard);
        released_at
    });
    acquired_rx
        .recv()
        .unwrap_or_else(|_| panic!("case {case:?}: the store lock was never taken"));
    handle
}

/// One row of the table: an entry point driven against a tunnel whose lifetime
/// runs out while the call waits for the store.
#[derive(Debug, Clone, Copy)]
struct Row {
    entry: Entry,
    /// The `clientMode` a rotating row asks for.
    client_mode: &'static str,
    /// Whether the tunnel carries the tag an untagging row removes.
    tagged: bool,
    /// The `maxLifetimeTimeoutMinutes` the expired lifetime came from.
    minutes: i64,
    /// The HTTP status the call itself answers: every entry point serves the
    /// request, except that no access token is minted for a tunnel that is no
    /// longer open.
    expected_status: u16,
    /// Whether the call changes something the tunnel reports, and so leaves a
    /// stamp of its own on the record.
    stamps: bool,
}

/// The rows: every entry point the service serves, over both whether the call
/// changes what the tunnel reports and the published bounds of the lifetime.
fn rows() -> [Row; 9] {
    [
        Row {
            // A key the tunnel does not carry: the tag set changes, so the
            // tunnel is updated and stamped.
            entry: Entry::TagResource,
            client_mode: "ALL",
            tagged: false,
            minutes: 1,
            expected_status: 200,
            stamps: true,
        },
        Row {
            // A key the tunnel carries, removed: again a change, again stamped.
            entry: Entry::UntagResource,
            client_mode: "ALL",
            tagged: true,
            minutes: 720,
            expected_status: 200,
            stamps: true,
        },
        Row {
            // The same removal against a tunnel that never carried the key:
            // nothing changes, so nothing is stamped.
            entry: Entry::UntagResource,
            client_mode: "ALL",
            tagged: false,
            minutes: 60,
            expected_status: 200,
            stamps: false,
        },
        Row {
            // The tunnel is already closed by the clock, so the explicit close
            // has nothing left to change.
            entry: Entry::CloseTunnel,
            client_mode: "ALL",
            tagged: true,
            minutes: 720,
            expected_status: 200,
            stamps: false,
        },
        Row {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "ALL",
            tagged: false,
            minutes: 1,
            expected_status: 400,
            stamps: false,
        },
        Row {
            entry: Entry::RotateTunnelAccessToken,
            client_mode: "SOURCE",
            tagged: true,
            minutes: 60,
            expected_status: 400,
            stamps: false,
        },
        Row {
            entry: Entry::DescribeTunnel,
            client_mode: "ALL",
            tagged: false,
            minutes: 720,
            expected_status: 200,
            stamps: false,
        },
        Row {
            entry: Entry::ListTagsForResource,
            client_mode: "ALL",
            tagged: true,
            minutes: 1,
            expected_status: 200,
            stamps: false,
        },
        Row {
            // Addresses the armed record through nothing at all, and still has
            // to leave its close alone once it is settled.
            entry: Entry::ListTunnels,
            client_mode: "ALL",
            tagged: false,
            minutes: 60,
            expected_status: 200,
            stamps: false,
        },
    ]
}

/// Drive ONE row through the rule.
///
/// The one place the rule is asserted, so both settling callers are held to the
/// same conditions and the two tables cannot drift on what the rule says.
async fn check_row(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: Row,
    settled_by: SettledBy,
) {
    let case = format!("{row:?}, close settled by {settled_by:?}");
    let (tunnel_id, tunnel_arn) = open_tunnel(router, &case, row.minutes).await;
    if row.tagged {
        tag_fixture(router, &tunnel_arn, &case).await;
    }
    let before = stored(engine, &tunnel_id, &case);
    assert_eq!(
        (before.status, before.closed_at),
        (TunnelStatus::Open, None),
        "case {case:?}: the fixture starts OPEN, got {:?}",
        (before.status, before.closed_at)
    );

    // Its lifetime runs out while the call below waits for the store.
    let expires_at = arm_expiry(engine, &tunnel_id, &case);
    let hold = hold_store_lock(engine, &case, settled_by);
    let (status, body) = invoke(
        router,
        row.entry,
        row.client_mode,
        &tunnel_id,
        &tunnel_arn,
        &case,
    )
    .await;
    let released_at = hold
        .join()
        .unwrap_or_else(|_| panic!("case {case:?}: the lock-holding thread panicked"));
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

    let after = stored(engine, &tunnel_id, &case);
    assert_eq!(
        (after.status, after.closed_at),
        (TunnelStatus::Closed, Some(expires_at)),
        "case {case:?}: the owed close is settled at the instant it came due, \
         expected CLOSED at {expires_at:?}, got {:?}",
        (after.status, after.closed_at)
    );
    // The rule: whoever settled the close, the record's stamp never precedes it.
    assert!(
        after.last_updated_at >= expires_at,
        "case {case:?}: lastUpdatedAt must never precede the close the clock owed \
         at {expires_at:?}, got {:?}",
        after.last_updated_at
    );
    if row.stamps {
        assert!(
            after.last_updated_at >= released_at,
            "case {case:?}: a call that changed what the tunnel reports stamps the instant \
             it held the store, expected at least {released_at:?}, got {:?}",
            after.last_updated_at
        );
    } else {
        assert_eq!(
            after.last_updated_at, expires_at,
            "case {case:?}: a call that changed nothing the tunnel reports leaves the close's \
             own stamp, expected {expires_at:?}, got {:?}",
            after.last_updated_at
        );
    }

    // What a client reads back carries the same ordering: the reported stamp is
    // not earlier than the close the same record reports.
    let (describe_status, described) =
        call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        describe_status, 200,
        "case {case:?}: DescribeTunnel answered {describe_status} {described:?}"
    );
    let tunnel = &described["tunnel"];
    assert_eq!(
        tunnel["status"].as_str(),
        Some("CLOSED"),
        "case {case:?}: the expired tunnel reports CLOSED, got {described:?}"
    );
    let reported = tunnel["lastUpdatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnel.lastUpdatedAt in {described:?}"));
    let closed_at = epoch_secs(expires_at, &case);
    assert!(
        reported + TIMESTAMP_TOLERANCE >= closed_at,
        "case {case:?}: the reported lastUpdatedAt must not precede the close instant \
         {closed_at}, got {reported}"
    );
    let created_at = tunnel["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnel.createdAt in {described:?}"));
    assert!(
        reported + TIMESTAMP_TOLERANCE >= created_at,
        "case {case:?}: the reported lastUpdatedAt must not precede createdAt {created_at}, \
         got {reported}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_call_that_waited_for_the_store_stamps_no_earlier_than_the_close_it_settled() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for row in rows() {
        check_row(&router, &engine, row, SettledBy::TheCallThatWaited).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stamp_never_precedes_a_close_another_caller_had_already_settled() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for row in rows() {
        check_row(&router, &engine, row, SettledBy::AnotherCallerThatHeldTheStore).await;
    }
}
