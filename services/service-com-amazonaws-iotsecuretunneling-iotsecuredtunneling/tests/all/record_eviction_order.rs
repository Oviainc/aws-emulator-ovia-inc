// SPDX-License-Identifier: CC0-1.0

//! Which record the store gives up when an `OpenTunnel` at the record bound has
//! to make room, asserted over a class of histories rather than over one.
//!
//! RULE: records are reclaimed in the order in which they reached `CLOSED` — the
//! record closed longest ago goes first, taking its tags with it — and an OPEN
//! record is never reclaimed. The close instant is a property of the close
//! transition, so no later write to a closed record (a tag put, a tag removal)
//! moves it in that order, however recently it stamped the tunnel's
//! `lastUpdatedAt`, which reports the last change of ANY kind rather than when
//! the record was closed.
//!
//! Rows differ along what the rule generalises over: the route by which a record
//! left `OPEN` (the explicit `CloseTunnel`, and the close the clock owes a tunnel
//! whose `maxLifetimeTimeoutMinutes` ran out), which later write touched a closed
//! record and which record it touched, and — in the second test — the number of
//! closed records waiting and the permutation between their creation order, their
//! close order and the order of the writes that stamped them.
//!
//! The bound itself is read from the crate rather than re-spelled here.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    StoredTunnel, MAX_RETAINED_TUNNELS,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region whose endpoint serves every request of these tests.
const REGION: &str = "eu-west-1";

/// The Region that holds the OPEN filler records which put the store at its
/// bound. A separate Region, so the records under test are the only ones of
/// `REGION` and the filler cannot be confused with them.
const FILLER_REGION: &str = "eu-filler-1";

/// How far apart the tests place two consecutive closes, so the close ORDER is
/// fixed by the test rather than by how far apart two wire calls land on the
/// clock.
const CLOSE_STEP: Duration = Duration::from_secs(600);

/// The tag every record under test carries from the moment it is opened, so an
/// eviction can be observed to take the record's tags with it and a row can
/// remove a key that is really there.
const SEEDED_TAG_KEY: &str = "env";
const SEEDED_TAG_VALUE: &str = "test";

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation against `REGION`'s endpoint the way an SDK does, and
/// return `(status, body)`.
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

/// One record under test, as the service handed it out.
#[derive(Debug, Clone)]
struct Record {
    tunnel_id: String,
    tunnel_arn: String,
}

/// Open one tunnel carrying [`SEEDED_TAG_KEY`], asserting the open is served.
async fn open(case: &str, router: &MultiServiceRouter) -> Record {
    let (status, body) = call(
        router,
        "OpenTunnel",
        &json!({
            "destinationConfig": {"thingName": "thing-eviction", "services": ["SSH"]},
            "tags": [{"key": SEEDED_TAG_KEY, "value": SEEDED_TAG_VALUE}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: OpenTunnel must be served, answered {status} {body:?}"
    );
    Record {
        tunnel_id: body["tunnelId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        tunnel_arn: body["tunnelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Close `tunnel_id` over the wire, keeping its record readable.
async fn close(case: &str, router: &MultiServiceRouter, tunnel_id: &str) {
    let (status, body) = call(router, "CloseTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CloseTunnel must be served, answered {status} {body:?}"
    );
}

/// The status `DescribeTunnel` reports for `tunnel_id`, or `None` when the
/// endpoint no longer holds the record.
async fn described_status(router: &MultiServiceRouter, tunnel_id: &str) -> Option<String> {
    let (status, body) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    if status != 200 {
        return None;
    }
    body["tunnel"]["status"].as_str().map(str::to_string)
}

/// Put one tag on `resource_arn`, asserting the write is served. This is what
/// stamps a closed record's `lastUpdatedAt` with the instant of the write.
async fn tag(case: &str, router: &MultiServiceRouter, resource_arn: &str, key: &str) {
    let (status, body) = call(
        router,
        "TagResource",
        &json!({"resourceArn": resource_arn, "tags": [{"key": key, "value": "fleet"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource must be served, answered {status} {body:?}"
    );
}

/// Remove one tag key from `resource_arn`, asserting the write is served.
async fn untag(case: &str, router: &MultiServiceRouter, resource_arn: &str, key: &str) {
    let (status, body) = call(
        router,
        "UntagResource",
        &json!({"resourceArn": resource_arn, "tagKeys": [key]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UntagResource must be served, answered {status} {body:?}"
    );
}

/// The tag keys `ListTagsForResource` reports for `resource_arn`, or `None` when
/// the endpoint holds no such resource (`ResourceNotFoundException`).
async fn listed_tag_keys(router: &MultiServiceRouter, resource_arn: &str) -> Option<Vec<String>> {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": resource_arn}),
    )
    .await;
    if status != 200 {
        return None;
    }
    Some(
        body["tags"]
            .as_array()
            .map(|tags| {
                tags.iter()
                    .filter_map(|entry| entry["key"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// How many records the store holds, over every Region.
fn stored_records(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine.state::<TunnelState>().lock().tunnels.len()
}

/// Put the store AT its record bound with OPEN filler records.
///
/// The records under test are opened over the wire; the filler only has to make
/// the store full, so it is written straight into the store — as this crate's
/// other bound suites seed their fillers — from a clone of a record the service
/// itself minted. Every filler is OPEN, so none of them is ever a candidate for
/// eviction.
fn fill_to_bound(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let seed: StoredTunnel = store
        .tunnels
        .values()
        .find(|tunnel| tunnel.closed_at.is_none())
        .cloned()
        .expect("the test opens a tunnel before filling the store");
    let mut filler = 0_usize;
    while store.tunnels.len() < MAX_RETAINED_TUNNELS {
        let tunnel_id = format!("filler-{filler}");
        let mut tunnel = seed.clone();
        tunnel.tunnel_arn = format!("arn:aws:iot:{FILLER_REGION}:000000000000:tunnel/{tunnel_id}");
        tunnel.tunnel_id = tunnel_id;
        tunnel.creation_seq = store.next_creation_seq();
        store.insert_tunnel(FILLER_REGION, tunnel);
        filler += 1;
    }
}

/// Move the whole close timeline of the closed record `tunnel_id` back by
/// `shift`: the instant it closed and the `lastUpdatedAt` that close stamped.
///
/// This is how a row says "this record was closed longer ago than that one"
/// without depending on the clock resolution between two wire calls.
fn rewind_close(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str, shift: Duration) {
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let tunnel = store
        .tunnel_mut(REGION, tunnel_id)
        .unwrap_or_else(|| panic!("the record {tunnel_id:?} was not persisted"));
    let closed_at = tunnel
        .closed_at
        .unwrap_or_else(|| panic!("the record {tunnel_id:?} is not closed"));
    tunnel.closed_at = Some(closed_at - shift);
    tunnel.last_updated_at -= shift;
}

/// Put the record `tunnel_id`'s lifetime in the past, so the close the clock
/// owes it is due, and keep the store's "soonest expiry" index consistent the
/// way `OpenTunnel` does.
fn expire_lifetime(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str) {
    let due = SystemTime::now() - Duration::from_secs(1);
    let state = engine.state::<TunnelState>();
    let mut store = state.lock();
    let tunnel = store
        .tunnel_mut(REGION, tunnel_id)
        .unwrap_or_else(|| panic!("the record {tunnel_id:?} was not persisted"));
    tunnel.expires_at = due;
    store.note_open_expiry(due);
}

/// The stored `lastUpdatedAt` of `tunnel_id`, so a row can assert that its later
/// write really did stamp the record it was supposed to stamp.
fn stored_last_updated_at(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
) -> Option<SystemTime> {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| tunnel.last_updated_at)
}

/// The route by which a record leaves `OPEN` in a row below.
#[derive(Debug, Clone, Copy)]
enum CloseRoute {
    /// `CloseTunnel` without `delete`: the data-preserving close.
    Explicit,
    /// The close the clock owes a tunnel whose `maxLifetimeTimeoutMinutes` ran
    /// out.
    LifetimeExpiry,
}

/// The write a row makes to a CLOSED record after it closed, which stamps that
/// record's `lastUpdatedAt` with the instant of the write.
#[derive(Debug, Clone, Copy)]
enum LaterWrite {
    /// Nothing is written after the closes.
    None,
    /// A tag is put on the record closed LONGEST ago.
    TagLongestClosed,
    /// A tag is removed from the record closed LONGEST ago.
    UntagLongestClosed,
    /// A tag is put on the record closed MOST RECENTLY.
    TagRecentlyClosed,
}

#[tokio::test]
async fn the_record_closed_longest_ago_is_evicted_whatever_was_written_to_it_afterwards() {
    // Rows: the route out of `OPEN`, and which closed record a later write
    // touched.
    let rows: [(CloseRoute, LaterWrite); 6] = [
        (CloseRoute::Explicit, LaterWrite::None),
        (CloseRoute::Explicit, LaterWrite::TagLongestClosed),
        (CloseRoute::Explicit, LaterWrite::UntagLongestClosed),
        (CloseRoute::Explicit, LaterWrite::TagRecentlyClosed),
        (CloseRoute::LifetimeExpiry, LaterWrite::None),
        (CloseRoute::LifetimeExpiry, LaterWrite::TagLongestClosed),
    ];

    for (route, later) in rows {
        let case = format!("{route:?} + {later:?}");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mount(&engine);

        // Two records that will be closed, and one that stays OPEN.
        let longest_closed = open(&case, &router).await;
        let recently_closed = open(&case, &router).await;
        let still_open = open(&case, &router).await;
        fill_to_bound(&engine);
        assert_eq!(
            stored_records(&engine),
            MAX_RETAINED_TUNNELS,
            "case {case:?}: the store must start AT its bound so one open has to reclaim"
        );

        // The first record leaves `OPEN` by this row's route, the second after it
        // by the explicit close.
        match route {
            CloseRoute::Explicit => close(&case, &router, &longest_closed.tunnel_id).await,
            CloseRoute::LifetimeExpiry => {
                expire_lifetime(&engine, &longest_closed.tunnel_id);
                assert_eq!(
                    described_status(&router, &longest_closed.tunnel_id)
                        .await
                        .as_deref(),
                    Some("CLOSED"),
                    "case {case:?}: a tunnel past its lifetime must be CLOSED"
                );
            }
        }
        close(&case, &router, &recently_closed.tunnel_id).await;
        // State the close ORDER instead of relying on the gap between two calls.
        rewind_close(&engine, &longest_closed.tunnel_id, CLOSE_STEP);

        // This row's write to a closed record.
        match later {
            LaterWrite::None => {}
            LaterWrite::TagLongestClosed => {
                tag(&case, &router, &longest_closed.tunnel_arn, "owner").await;
            }
            LaterWrite::UntagLongestClosed => {
                untag(
                    &case,
                    &router,
                    &longest_closed.tunnel_arn,
                    SEEDED_TAG_KEY,
                )
                .await;
            }
            LaterWrite::TagRecentlyClosed => {
                tag(&case, &router, &recently_closed.tunnel_arn, "owner").await;
            }
        }

        // The rows that write to the longest-closed record are exactly the ones
        // where `lastUpdatedAt` now orders the two records the other way round,
        // so each of them really is the divergent history.
        let longest_stamp = stored_last_updated_at(&engine, &longest_closed.tunnel_id);
        let recent_stamp = stored_last_updated_at(&engine, &recently_closed.tunnel_id);
        let stamped_last = matches!(
            later,
            LaterWrite::TagLongestClosed | LaterWrite::UntagLongestClosed
        );
        assert_eq!(
            longest_stamp > recent_stamp,
            stamped_last,
            "case {case:?}: a write to the longest-closed record must leave it with the newer \
             lastUpdatedAt, got {longest_stamp:?} against {recent_stamp:?}"
        );

        // One more open, which the full store has to make room for.
        let fresh = open(&case, &router).await;

        let evicted = described_status(&router, &longest_closed.tunnel_id).await;
        assert!(
            evicted.is_none(),
            "case {case:?}: the record closed longest ago must be the one evicted, \
             DescribeTunnel reported {evicted:?}"
        );
        assert_eq!(
            listed_tag_keys(&router, &longest_closed.tunnel_arn).await,
            None,
            "case {case:?}: the evicted record's tags must leave the store with it"
        );
        assert_eq!(
            described_status(&router, &recently_closed.tunnel_id)
                .await
                .as_deref(),
            Some("CLOSED"),
            "case {case:?}: the record closed more recently must survive the eviction"
        );
        assert_eq!(
            listed_tag_keys(&router, &recently_closed.tunnel_arn).await,
            Some(match later {
                LaterWrite::TagRecentlyClosed =>
                    vec![SEEDED_TAG_KEY.to_string(), "owner".to_string()],
                _ => vec![SEEDED_TAG_KEY.to_string()],
            }),
            "case {case:?}: the surviving record must keep its own tags"
        );
        assert_eq!(
            described_status(&router, &still_open.tunnel_id)
                .await
                .as_deref(),
            Some("OPEN"),
            "case {case:?}: an OPEN record is never evicted"
        );
        assert_eq!(
            described_status(&router, &fresh.tunnel_id).await.as_deref(),
            Some("OPEN"),
            "case {case:?}: the open that reclaimed the record must be stored"
        );
        assert_eq!(
            stored_records(&engine),
            MAX_RETAINED_TUNNELS,
            "case {case:?}: the store must stay within its bound of {MAX_RETAINED_TUNNELS} records"
        );
    }
}

#[tokio::test]
async fn successive_evictions_walk_the_closed_records_in_close_order() {
    // Four records whose close order is neither their creation order nor the
    // order of the writes that stamped their `lastUpdatedAt`: each eviction must
    // take the record closed longest ago of those still held.
    //
    // `shift` is how far back each record's close is moved, so the record with
    // the LARGEST shift closed longest ago.
    let close_shifts = [1_u32, 4, 2, 3];
    let case = "four closed records, close order a permutation of creation order";

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let mut records = Vec::new();
    for _ in 0..close_shifts.len() {
        records.push(open(case, &router).await);
    }
    let still_open = open(case, &router).await;
    fill_to_bound(&engine);

    // Close every record, then state the close order through the shifts.
    for record in &records {
        close(case, &router, &record.tunnel_id).await;
    }
    for (record, shift) in records.iter().zip(close_shifts) {
        rewind_close(&engine, &record.tunnel_id, CLOSE_STEP * shift);
    }

    // The expected eviction order: closed longest ago (largest shift) first.
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(close_shifts[index]));
    let expected: Vec<&Record> = order.iter().map(|&index| &records[index]).collect();

    // Stamp `lastUpdatedAt` in the REVERSE of the expected eviction order, so the
    // record evicted first is the one whose stamp is newest.
    for record in expected.iter().rev() {
        tag(case, &router, &record.tunnel_arn, "owner").await;
    }

    for (step, record) in expected.iter().enumerate() {
        let fresh = open(case, &router).await;
        assert_eq!(
            described_status(&router, &fresh.tunnel_id).await.as_deref(),
            Some("OPEN"),
            "case {case:?}: eviction {step}: the open that reclaimed a record must be stored"
        );
        let evicted = described_status(&router, &record.tunnel_id).await;
        assert!(
            evicted.is_none(),
            "case {case:?}: eviction {step} must take {:?}, the record closed longest ago of \
             those left, but DescribeTunnel reported {evicted:?}",
            record.tunnel_id
        );
        for later in expected.iter().skip(step + 1) {
            assert_eq!(
                described_status(&router, &later.tunnel_id).await.as_deref(),
                Some("CLOSED"),
                "case {case:?}: eviction {step} must not take {:?}, which was closed later",
                later.tunnel_id
            );
        }
        assert_eq!(
            described_status(&router, &still_open.tunnel_id)
                .await
                .as_deref(),
            Some("OPEN"),
            "case {case:?}: eviction {step} must not take the OPEN record"
        );
        assert_eq!(
            stored_records(&engine),
            MAX_RETAINED_TUNNELS,
            "case {case:?}: eviction {step}: the store must stay within its bound"
        );
    }
}
