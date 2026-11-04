// SPDX-License-Identifier: CC0-1.0

//! What the tunnel quota counts, end to end.
//!
//! RULE: the quota the modelled `LimitExceededException` reports is a
//! quota on CONCURRENCY — a tunnel occupies one of the Region's
//! `state::MAX_CONCURRENT_TUNNELS` slots only while its status is `OPEN`, so a
//! tunnel that has reached `CLOSED` by any route releases its slot at once even
//! when its record stays describable, and the next `OpenTunnel` succeeds; a
//! Region's slots are its own.
//!
//! Rows differ along what the rule generalises over: how the tunnel left `OPEN`
//! (a plain close, an explicit `delete: false`, a close with `delete: true` that
//! drops the record, and the clock-owed close when the tunnel's lifetime runs
//! out), whether the record survives the transition, and which Region holds the
//! tunnel. The quota itself is read from the crate rather than re-spelled here.
//!
//! The store's own record bound (`state::MAX_RETAINED_TUNNELS`) is asserted here
//! too, because it is not a quota a caller is subject to: it holds over every
//! Region at once, it gives up the longest-closed record before it refuses, and
//! it never evicts an open tunnel.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    MAX_CONCURRENT_TUNNELS, MAX_RETAINED_TUNNELS,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the table's tunnels are opened in.
const REGION: &str = "eu-west-1";

/// A second Region, so a row can show that slots are not shared between them.
const OTHER_REGION: &str = "us-west-2";

/// The wire code of this service's limit refusal — the modelled shape's own name,
/// `com.amazonaws.iotsecuretunneling#LimitExceededException` — and the status it
/// arrives with: the `awsJson1_1` client-error status, since the model's
/// `@httpError` is inert on this service's protocol.
const LIMIT_CODE: &str = "LimitExceededException";
const LIMIT_STATUS: u16 = 400;

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation against `region`'s endpoint the way an SDK does, and
/// return `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/IoTSecuredTunneling/\
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

/// Open one tunnel in `region` and return `(status, body)`.
async fn open(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call(
        router,
        region,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": "thing-quota", "services": ["SSH"]}}),
    )
    .await
}

/// Open `region`'s whole quota of tunnels and return their ids, asserting every
/// open up to the quota is served.
async fn fill_quota(case: &str, router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let mut ids = Vec::with_capacity(MAX_CONCURRENT_TUNNELS);
    for index in 0..MAX_CONCURRENT_TUNNELS {
        let (status, body) = open(router, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: open {index} of the quota must be served, answered {status} {body:?}"
        );
        ids.push(body["tunnelId"].as_str().unwrap_or_default().to_string());
    }
    ids
}

/// Assert that `status`/`body` is this service's modelled limit refusal and that
/// its message names the quota.
fn assert_quota_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, LIMIT_STATUS,
        "case {case:?}: expected the modelled limit refusal, answered {status} {body:?}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(LIMIT_CODE),
        "case {case:?}: expected the modelled limit error code, answered {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&MAX_CONCURRENT_TUNNELS.to_string()),
        "case {case:?}: the message must name the quota {MAX_CONCURRENT_TUNNELS}, got {message:?}"
    );
    for field in [
        "tunnelId",
        "tunnelArn",
        "sourceAccessToken",
        "destinationAccessToken",
    ] {
        assert!(
            body.get(field).is_none(),
            "case {case:?}: a refused open must hand out no {field}, got {:?}",
            body.get(field)
        );
    }
}

/// The status `DescribeTunnel` reports for `tunnel_id`, or `None` when the
/// Region's endpoint no longer holds the record.
async fn described_status(
    router: &MultiServiceRouter,
    region: &str,
    tunnel_id: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        region,
        "DescribeTunnel",
        &json!({"tunnelId": tunnel_id}),
    )
    .await;
    if status != 200 {
        return None;
    }
    body["tunnel"]["status"].as_str().map(str::to_string)
}

/// How a tunnel leaves `OPEN` in one row of the table below.
#[derive(Debug, Clone, Copy)]
enum Release {
    /// `CloseTunnel` with `delete` absent: the documented data-preserving close.
    Close,
    /// `CloseTunnel` with the flag spelled out as `false`.
    CloseDeleteFalse,
    /// `CloseTunnel` with `delete: true`, which also drops the record.
    CloseAndDelete,
    /// The clock-owed close: the tunnel's `maxLifetimeTimeoutMinutes` ran out.
    LifetimeExpiry,
}

#[tokio::test]
async fn only_tunnels_that_are_open_consume_the_concurrency_quota() {
    // Rows: every route out of `OPEN`, and whether the record survives it.
    let rows: [(Release, Option<&str>); 4] = [
        (Release::Close, Some("CLOSED")),
        (Release::CloseDeleteFalse, Some("CLOSED")),
        (Release::CloseAndDelete, None),
        (Release::LifetimeExpiry, Some("CLOSED")),
    ];

    for (release, expected_status) in rows {
        let case = format!("{release:?}");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mount(&engine);

        let ids = fill_quota(&case, &router, REGION).await;
        let released = ids.first().expect("the quota is at least one tunnel").clone();

        // At the quota, every further open is refused, and the refusal is not a
        // one-off: nothing has been released yet.
        for attempt in 0..2 {
            let (status, body) = open(&router, REGION).await;
            assert_quota_refusal(&format!("{case}: attempt {attempt} at the quota"), status, &body);
        }

        // Release exactly one slot, by this row's route.
        match release {
            Release::Close => {
                let (status, body) = call(
                    &router,
                    REGION,
                    "CloseTunnel",
                    &json!({"tunnelId": released}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CloseTunnel answered {status} {body:?}"
                );
            }
            Release::CloseDeleteFalse => {
                let (status, body) = call(
                    &router,
                    REGION,
                    "CloseTunnel",
                    &json!({"tunnelId": released, "delete": false}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CloseTunnel answered {status} {body:?}"
                );
            }
            Release::CloseAndDelete => {
                let (status, body) = call(
                    &router,
                    REGION,
                    "CloseTunnel",
                    &json!({"tunnelId": released, "delete": true}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CloseTunnel answered {status} {body:?}"
                );
            }
            Release::LifetimeExpiry => {
                // Put this one tunnel's lifetime in the past, so the close the
                // clock owes it is due; every other tunnel stays open.
                let due = SystemTime::now() - Duration::from_secs(1);
                let state = engine.state::<TunnelState>();
                let mut store = state.lock();
                let tunnel = store
                    .tunnel_mut(REGION, &released)
                    .expect("the opened tunnel was not persisted");
                tunnel.expires_at = due;
                store.note_open_expiry(due);
            }
        }

        // The record is readable exactly as this row expects — and either way it
        // holds no slot.
        let status_now = described_status(&router, REGION, &released).await;
        assert_eq!(
            status_now.as_deref(),
            expected_status,
            "case {case:?}: DescribeTunnel reported {status_now:?}"
        );

        let (status, body) = open(&router, REGION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the freed slot must let the next open through, answered {status} \
             {body:?}"
        );
        assert!(
            body["tunnelId"].as_str().is_some(),
            "case {case:?}: the served open answered {body:?}"
        );

        // And the quota is a live count, not a latch: the account is full again.
        let (status, body) = open(&router, REGION).await;
        assert_quota_refusal(&format!("{case}: full again"), status, &body);
    }
}

#[tokio::test]
async fn the_store_holds_a_bounded_number_of_records_across_every_region() {
    // The Region a record belongs to comes from the request, so the record count
    // is bounded over the whole store: filling one Region's quota and moving on
    // to the next must not keep growing it past `MAX_RETAINED_TUNNELS`.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let stored = || engine.state::<TunnelState>().lock().tunnels.len();
    let mut first: Option<(String, String)> = None;
    let mut filled = 0_usize;
    while stored() < MAX_RETAINED_TUNNELS {
        let region = format!("eu-fill-{filled}");
        let ids = fill_quota(&format!("filling {region}"), &router, &region).await;
        if first.is_none() {
            first = Some((region.clone(), ids[0].clone()));
        }
        filled += 1;
        assert!(
            stored() <= MAX_RETAINED_TUNNELS,
            "case {region:?}: the store must never hold more than {MAX_RETAINED_TUNNELS} records, \
             holds {}",
            stored()
        );
    }
    let (released_region, released_id) = first.expect("at least one Region was filled");

    // Every record is OPEN, so there is nothing retention may reclaim: the open
    // is refused rather than stored, whichever Region asks for it.
    let case = "a full store, every record open";
    let (status, body) = open(&router, "eu-fresh-region").await;
    assert_eq!(
        status, LIMIT_STATUS,
        "case {case:?}: expected the modelled limit refusal, answered {status} {body:?}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(LIMIT_CODE),
        "case {case:?}: expected the modelled limit error code, answered {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&MAX_RETAINED_TUNNELS.to_string()),
        "case {case:?}: the message must name the record limit {MAX_RETAINED_TUNNELS}, got \
         {message:?}"
    );
    assert_eq!(
        stored(),
        MAX_RETAINED_TUNNELS,
        "case {case:?}: a refused open must store no record"
    );

    // Once one record is CLOSED, it is the history the store gives up to make
    // room: the next open is served, the closed record is gone, and the count is
    // still bounded.
    let case = "a full store holding one closed record";
    let (status, body) = call(
        &router,
        &released_region,
        "CloseTunnel",
        &json!({"tunnelId": released_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CloseTunnel answered {status} {body:?}"
    );
    let (status, body) = open(&router, "eu-fresh-region").await;
    assert_eq!(
        status, 200,
        "case {case:?}: the open must be served once a record can be reclaimed, answered {status} \
         {body:?}"
    );
    assert!(
        described_status(&router, &released_region, &released_id)
            .await
            .is_none(),
        "case {case:?}: the longest-closed record must be the one evicted"
    );
    assert_eq!(
        stored(),
        MAX_RETAINED_TUNNELS,
        "case {case:?}: the store must stay within {MAX_RETAINED_TUNNELS} records, holds {}",
        stored()
    );
}

#[tokio::test]
async fn each_region_holds_its_own_tunnel_slots() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let case = "one Region at its quota";
    fill_quota(case, &router, REGION).await;

    let (status, body) = open(&router, REGION).await;
    assert_quota_refusal(case, status, &body);

    // Another Region's endpoint holds its own slots, so it serves the open the
    // first Region refuses.
    let (status, body) = open(&router, OTHER_REGION).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {OTHER_REGION} must serve an open of its own, answered {status} {body:?}"
    );
    let opened = body["tunnelId"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        described_status(&router, OTHER_REGION, &opened)
            .await
            .as_deref(),
        Some("OPEN"),
        "case {case:?}: the tunnel opened in {OTHER_REGION} must be readable there"
    );
    assert!(
        described_status(&router, REGION, &opened).await.is_none(),
        "case {case:?}: a tunnel of {OTHER_REGION} must not be readable through {REGION}"
    );
}
