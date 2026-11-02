// SPDX-License-Identifier: CC0-1.0

//! The order `ListTunnels` pages in never contradicts the `createdAt` it reports.
//!
//! RULE: a tunnel's creation instant and its place in creation order are claimed
//! in ONE critical section of the store, so the order the listing reports —
//! "Tunnels are listed by creation time in descending order, newer tunnels will
//! be listed before older tunnels"
//! (`smithy_query(query="doc",shape="ListTunnels",model_path="services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json")`)
//! — agrees with the `createdAt` values the same response carries, however many
//! opens are in flight at once: within a page and across pages, the reported
//! `createdAt` is non-increasing, so a client that re-sorts a page by the
//! `createdAt` it was handed gets back the order the service paged it in.
//!
//! Rows differ along what the rule generalises over: how many `OpenTunnel` calls
//! are in flight at once, the page size the listing is read at, and whether the
//! tunnels are split across two `thingName`s and read back through that filter.
//!
//! The last test pins the mechanism the rule rests on. Acquiring this service's
//! single store lock blocks for an unbounded time, so a stamp taken while a
//! request was still QUEUED for the store would describe an instant before the
//! store was free, while the creation order is claimed once the lock is held:
//! every tunnel opened while another thread held the lock is therefore stamped at
//! or after the instant that hold ended.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::MAX_CONCURRENT_TUNNELS;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call in this suite is signed for.
const REGION: &str = "ap-south-1";

/// The two `thingName`s the table's tunnels are opened against, so a row can read
/// the same listing back through the filter the operation models.
const THING_A: &str = "thing-race-a";
const THING_B: &str = "thing-race-b";

/// How long the store lock is held from another thread in the last test. Far
/// longer than the work an open does under the lock, so a stamp taken before the
/// acquisition and one taken after it are never confusable.
const HOLD: Duration = Duration::from_millis(400);

/// A ceiling on how many summaries one walk of a listing may collect, so a token
/// that failed to advance ends the test instead of paging for ever. Twice the
/// tunnels a Region can hold open, which is more than any row opens.
const PAGE_WALK_LIMIT: usize = MAX_CONCURRENT_TUNNELS * 2;

/// A router carrying only this service, over the engine the test also inspects.
/// Shared, because the concurrent rows dispatch through it from several tasks.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> Arc<MultiServiceRouter> {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    Arc::new(router)
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

/// Open one tunnel against `thing_name` and answer its `tunnelId`.
async fn open_tunnel(router: &MultiServiceRouter, thing_name: &str, case: &str) -> String {
    let (status, opened) = call(
        router,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": thing_name, "services": ["SSH"]}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: OpenTunnel answered {status} {opened:?}"
    );
    opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
        .to_string()
}

/// One reported summary: the tunnel it names and the `createdAt` it carried.
#[derive(Debug, Clone)]
struct Reported {
    tunnel_id: String,
    /// The wire value, which `awsJson1_1` carries as epoch seconds.
    created_at: f64,
}

/// Assert that `reported` carries its `createdAt` values in non-increasing
/// order — the claim the listing's documented order makes to the client.
fn assert_newest_first(case: &str, reported: &[Reported]) {
    for pair in reported.windows(2) {
        let (newer, older) = (&pair[0], &pair[1]);
        assert!(
            newer.created_at >= older.created_at,
            "case {case:?}: the listing reports newer tunnels first, so no summary may carry a \
             createdAt earlier than the one after it, got {newer:?} before {older:?}"
        );
    }
}

/// Read a whole listing, following `nextToken` until the last page, and answer
/// every summary in the order it was paged in.
async fn listing(
    router: &MultiServiceRouter,
    thing_name: Option<&str>,
    page_size: Option<i64>,
    case: &str,
) -> Vec<Reported> {
    let mut reported: Vec<Reported> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut input = json!({});
        if let Some(thing_name) = thing_name {
            input["thingName"] = json!(thing_name);
        }
        if let Some(page_size) = page_size {
            input["maxResults"] = json!(page_size);
        }
        if let Some(token) = next_token.as_deref() {
            input["nextToken"] = json!(token);
        }

        let (status, listed) = call(router, "ListTunnels", &input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTunnels {input:?} answered {status} {listed:?}"
        );
        let page: Vec<Reported> = listed["tunnelSummaries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|summary| Reported {
                        tunnel_id: summary["tunnelId"]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: summary {summary:?} names no tunnel")
                            })
                            .to_string(),
                        created_at: summary["createdAt"].as_f64().unwrap_or_else(|| {
                            panic!("case {case:?}: summary {summary:?} reports no createdAt")
                        }),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(page_size) = page_size {
            let size = i64::try_from(page.len()).unwrap_or(i64::MAX);
            assert!(
                size <= page_size,
                "case {case:?}: a page holds at most the maxResults asked for, expected at most \
                 {page_size}, got {size}"
            );
        }
        // Each page is newest-first in itself, and the pages join into one
        // newest-first listing: the order is asserted after every page, so a
        // contradiction at a page boundary is caught where it happens.
        assert_newest_first(case, &page);
        reported.extend(page);
        assert_newest_first(case, &reported);
        assert!(
            reported.len() <= PAGE_WALK_LIMIT,
            "case {case:?}: the walk collected {} summaries, past the {PAGE_WALK_LIMIT} any row \
             opens",
            reported.len()
        );

        next_token = listed["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return reported;
        }
    }
}

/// The stored creation instant of `tunnel_id`, at the full precision the record
/// holds it in, read straight out of the store.
fn stored_created_at(
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    case: &str,
) -> SystemTime {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| tunnel.created_at)
        .unwrap_or_else(|| panic!("case {case:?}: tunnel {tunnel_id} is not in the store"))
}

/// The `createdAt` `DescribeTunnel` reports for `tunnel_id`.
async fn described_created_at(router: &MultiServiceRouter, tunnel_id: &str, case: &str) -> f64 {
    let (status, body) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeTunnel {tunnel_id} answered {status} {body:?}"
    );
    body["tunnel"]["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeTunnel reported no createdAt in {body:?}"))
}

/// One row of the table: a number of simultaneous opens, the page size the
/// listing is read at, and whether the opens are split across two things.
#[derive(Debug, Clone, Copy)]
struct Row {
    concurrency: usize,
    /// The `maxResults` the paged read asks for. `MaxResults` is
    /// `range(min: 1, max: 100)`.
    page_size: i64,
    split: bool,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opens_are_paged_in_the_order_of_the_created_at_they_report() {
    let rows = [
        Row {
            concurrency: 1,
            page_size: 1,
            split: false,
        },
        Row {
            concurrency: 2,
            page_size: 1,
            split: false,
        },
        Row {
            concurrency: 8,
            page_size: 3,
            split: true,
        },
        Row {
            concurrency: 24,
            page_size: 7,
            split: false,
        },
        // The most tunnels a Region may hold open at once, read as one page.
        Row {
            concurrency: MAX_CONCURRENT_TUNNELS,
            page_size: 100,
            split: true,
        },
    ];

    for row in rows {
        let case = format!("{row:?}");
        assert!(
            row.concurrency <= MAX_CONCURRENT_TUNNELS,
            "case {case:?}: a row may not ask for more open tunnels than the Region's quota of \
             {MAX_CONCURRENT_TUNNELS}"
        );
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mount(&engine);

        // Every open is in flight at once, so which of them reaches the store
        // first is the runtime's to choose and not the order they were issued in.
        let mut opening = Vec::with_capacity(row.concurrency);
        for index in 0..row.concurrency {
            let router = Arc::clone(&router);
            let case = case.clone();
            let thing_name = if row.split && index % 2 == 1 {
                THING_B
            } else {
                THING_A
            };
            opening.push(tokio::spawn(async move {
                (thing_name, open_tunnel(&router, thing_name, &case).await)
            }));
        }
        let mut thing_of: BTreeMap<String, &str> = BTreeMap::new();
        for task in opening {
            let (thing_name, tunnel_id) = task
                .await
                .unwrap_or_else(|_| panic!("case {case:?}: an OpenTunnel task panicked"));
            assert!(
                thing_of.insert(tunnel_id.clone(), thing_name).is_none(),
                "case {case:?}: tunnel {tunnel_id} was handed out twice"
            );
        }

        // One page holds every tunnel this row opened, and nothing else.
        let unpaged = listing(&router, None, None, &case).await;
        let mut listed_ids: Vec<&str> = unpaged
            .iter()
            .map(|entry| entry.tunnel_id.as_str())
            .collect();
        listed_ids.sort_unstable();
        let mut opened_ids: Vec<&str> = thing_of.keys().map(String::as_str).collect();
        opened_ids.sort_unstable();
        assert_eq!(
            listed_ids, opened_ids,
            "case {case:?}: the listing must report exactly the tunnels that were opened"
        );

        // The claim the response makes to the client: its own createdAt values
        // are in the order it paged them in.
        assert_newest_first(&case, &unpaged);

        // And the stamps the store holds, at the full precision the wire value is
        // rendered from, are in that same order: the creation order the listing
        // sorts by cannot disagree with the instant each record was stamped at.
        let stamps: Vec<SystemTime> = unpaged
            .iter()
            .map(|entry| stored_created_at(&engine, &entry.tunnel_id, &case))
            .collect();
        for (position, pair) in stamps.windows(2).enumerate() {
            assert!(
                pair[0] >= pair[1],
                "case {case:?}: the tunnel listed at {position} was created at {:?}, before the \
                 one listed after it at {:?}",
                pair[0],
                pair[1]
            );
        }

        // What the listing reports, a read of the same tunnel reports.
        for entry in &unpaged {
            let described = described_created_at(&router, &entry.tunnel_id, &case).await;
            assert_eq!(
                described, entry.created_at,
                "case {case:?}: tunnel {} is listed with createdAt {} and described with \
                 {described}",
                entry.tunnel_id, entry.created_at
            );
        }

        // Paging the same listing hands the client the same order, token by
        // token, since the token is a position in that order.
        let paged = listing(&router, None, Some(row.page_size), &case).await;
        let paged_ids: Vec<&str> = paged
            .iter()
            .map(|entry| entry.tunnel_id.as_str())
            .collect();
        let whole_ids: Vec<&str> = unpaged
            .iter()
            .map(|entry| entry.tunnel_id.as_str())
            .collect();
        assert_eq!(
            paged_ids, whole_ids,
            "case {case:?}: a listing read in pages of {} must report the order one page does",
            row.page_size
        );

        // A filtered listing is that same order narrowed to one thingName.
        if row.split {
            for thing_name in [THING_A, THING_B] {
                let filtered = listing(&router, Some(thing_name), Some(row.page_size), &case).await;
                assert_newest_first(&format!("{case}, filtered by {thing_name}"), &filtered);
                let filtered_ids: Vec<&str> = filtered
                    .iter()
                    .map(|entry| entry.tunnel_id.as_str())
                    .collect();
                let expected: Vec<&str> = whole_ids
                    .iter()
                    .copied()
                    .filter(|tunnel_id| thing_of.get(*tunnel_id).copied() == Some(thing_name))
                    .collect();
                assert_eq!(
                    filtered_ids, expected,
                    "case {case:?}: the listing filtered by {thing_name} must report the \
                     unfiltered order narrowed to that thing"
                );
            }
        }
    }
}

/// The store lock, held from another thread until it has been held for [`HOLD`].
/// `released_at` answers the instant it was let go, which every call that queued
/// behind it only reaches the store after.
struct LockHold {
    handle: JoinHandle<SystemTime>,
}

impl LockHold {
    fn released_at(self, case: &str) -> SystemTime {
        self.handle
            .join()
            .unwrap_or_else(|_| panic!("case {case:?}: the lock-holding thread panicked"))
    }
}

/// Take the store lock from another thread and keep it for [`HOLD`]. Returns once
/// the lock is actually held, so the requests issued next are guaranteed to queue
/// behind it rather than race it.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tunnel_queued_for_the_store_is_stamped_from_inside_its_own_critical_section() {
    // Rows differ in how many opens queue behind the one hold at once.
    for queued in [1usize, 2, 5] {
        let case = format!("{queued} opens queued behind the store lock");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = mount(&engine);

        // One tunnel opened before the hold: every tunnel opened after it is
        // created later, so it is the oldest of the listing.
        let earlier = open_tunnel(&router, THING_A, &case).await;
        let earlier_stamp = stored_created_at(&engine, &earlier, &case);

        let hold = hold_store_lock(&engine, &case);
        let mut opening = Vec::with_capacity(queued);
        for _ in 0..queued {
            let router = Arc::clone(&router);
            let case = case.clone();
            opening.push(tokio::spawn(async move {
                open_tunnel(&router, THING_A, &case).await
            }));
        }
        let mut opened: Vec<String> = Vec::with_capacity(queued);
        for task in opening {
            opened.push(
                task.await
                    .unwrap_or_else(|_| panic!("case {case:?}: an OpenTunnel task panicked")),
            );
        }
        let released_at = hold.released_at(&case);
        assert!(
            released_at > earlier_stamp,
            "case {case:?}: the hold must end after the earlier tunnel was created, released at \
             {released_at:?}, created at {earlier_stamp:?}"
        );

        for tunnel_id in &opened {
            let stamp = stored_created_at(&engine, tunnel_id, &case);
            assert!(
                stamp >= released_at,
                "case {case:?}: tunnel {tunnel_id} is stamped with the instant its open held the \
                 store, expected at least {released_at:?}, got {stamp:?}"
            );
            assert!(
                stamp > earlier_stamp,
                "case {case:?}: tunnel {tunnel_id} was created after {earlier:?}, expected a \
                 stamp later than {earlier_stamp:?}, got {stamp:?}"
            );
        }

        // Read one summary at a time, so the order is the one the page tokens
        // walk: the tunnel created before the hold is the last of them.
        let listed = listing(&router, None, Some(1), &case).await;
        assert_eq!(
            listed.len(),
            queued + 1,
            "case {case:?}: expected every opened tunnel reported, got {listed:?}"
        );
        assert_newest_first(&case, &listed);
        assert_eq!(
            listed.last().map(|entry| entry.tunnel_id.as_str()),
            Some(earlier.as_str()),
            "case {case:?}: the oldest tunnel is paged last, got {listed:?}"
        );
    }
}
