// SPDX-License-Identifier: CC0-1.0

//! The two classes of the `GetConnections` reporting contract the other suites of this
//! target leave unpinned, driven end-to-end over HTTP.
//!
//! What the siblings already cover: `connection_lifecycle` pins the stored record, the
//! narrowing on either end of a connection and the walk over every accepted page size;
//! `filter_id_list_maximum` pins the `ConnectionIds` bound alongside the other id-list
//! filters of this service; `global_network_lifecycle` pins the refusal of a continuation
//! token this service did not mint, `GetConnections` included.
//!
//! What this suite adds:
//!
//! * `MaxResults` is modelled `range 1..500`, so a page size INSIDE that range is
//!   honoured and one outside it is refused with the modelled `ValidationException`
//!   (HTTP 400) carrying no page at all — the rows span each published bound, one inside
//!   each and values past each. What is NEW here is the refusal outside the range (0, -1,
//!   -500, 501, 1000) and the exact content of the FIRST page at each accepted size; the
//!   accepted rows deliberately overlap `connection_lifecycle`'s walk, which follows the
//!   tokens to exhaustion instead.
//! * the lifecycle `State` the answer carries is a function of the clock ALONE: a fresh
//!   connection reports `PENDING` until the span the create owes has passed and
//!   `AVAILABLE` after it, and repeating the listing never moves it, filtered or not —
//!   `connection_lifecycle` compares records with `State` stripped precisely so as not to
//!   race that transition, so nothing pins it.
//!
//! Nothing here reaches into `crate::state`: every resource is created and read back
//! through an operation. The harness is the one `global_network_lifecycle` defines, and
//! the `GetConnections` helpers are `connection_lifecycle`'s.

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, get_connections, id_of, ids_in};
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The modelled bounds of `MaxResults` (`range 1..500`, `smithy_query query="operation"
/// operation="GetConnections"`).
const MIN_MAX_RESULTS: i64 = 1;
/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 500;

/// A global network holding two devices, which is everything a connection needs.
struct Pair {
    network: String,
    device: String,
    connected_device: String,
}

/// Create a global network with two devices in it.
async fn pair(emulator: &Emulator, label: &str) -> Pair {
    let network = emulator.create_global_network(label).await;
    Pair {
        device: create_device_ok(emulator, &network, json!({ "Description": "near" })).await,
        connected_device: create_device_ok(emulator, &network, json!({ "Description": "far" }))
            .await,
        network,
    }
}

/// `CreateConnection` between the two devices of `seed`, answering the minted id.
async fn connect(emulator: &Emulator, seed: &Pair) -> String {
    id_of(
        &create_connection_ok(
            emulator,
            &seed.network,
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
            }),
        )
        .await,
    )
}

/// The `State`s of one `GetConnections` answer, in the order returned.
fn states_in(body: &Value) -> Vec<String> {
    body["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Connections array in {body}"))
        .iter()
        .map(|connection| {
            connection["State"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a State in {connection}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn max_results_is_honoured_inside_its_modelled_range_and_refused_outside_it() {
    let emulator = Emulator::new();
    let seed = pair(&emulator, "paging-bounds").await;
    let held = [
        connect(&emulator, &seed).await,
        connect(&emulator, &seed).await,
        connect(&emulator, &seed).await,
    ];

    // The rows differ along the page size: each published bound, one inside each, sizes
    // either side of the record count, one past each bound and far past each.
    let rows: Vec<(&str, String, u16)> = vec![
        ("the low bound", MIN_MAX_RESULTS.to_string(), 200),
        ("one inside the low bound", "2".to_string(), 200),
        ("the record count", held.len().to_string(), 200),
        ("one past the record count", (held.len() + 1).to_string(), 200),
        (
            "one inside the high bound",
            (MAX_MAX_RESULTS - 1).to_string(),
            200,
        ),
        ("the high bound", MAX_MAX_RESULTS.to_string(), 200),
        ("one below the low bound", "0".to_string(), 400),
        ("negative", "-1".to_string(), 400),
        ("far below the low bound", "-500".to_string(), 400),
        (
            "one past the high bound",
            (MAX_MAX_RESULTS + 1).to_string(),
            400,
        ),
        ("far past the high bound", "1000".to_string(), 400),
    ];

    for (case, page_size, expected) in rows {
        let (status, body) =
            get_connections(&emulator, &seed.network, &format!("maxResults={page_size}")).await;
        assert_eq!(
            status, expected,
            "case {case} (maxResults={page_size}): expected {expected}, got {status} with {body}"
        );

        if expected == 400 {
            // Outside the modelled range nothing is listed at all.
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected a ValidationException, got {body}"
            );
            assert!(
                body["Connections"].is_null(),
                "case {case}: a refused request must carry no page, got {body}"
            );
            continue;
        }

        // Inside it the page holds the first `maxResults` records in key order, and a
        // token follows only while records remain.
        let page = ids_in(&body);
        let wanted = page_size
            .parse::<usize>()
            .unwrap_or_else(|error| panic!("case {case}: {page_size} is not a size: {error}"));
        let expected_page: Vec<String> = held.iter().take(wanted).map(String::clone).collect();
        assert_eq!(
            page, expected_page,
            "case {case}: expected {expected_page:?}, got {body}"
        );
        assert_eq!(
            body["NextToken"].is_string(),
            wanted < held.len(),
            "case {case}: a token must follow exactly while records remain, got {body}"
        );
    }
}

#[tokio::test]
async fn the_reported_state_follows_the_clock_and_a_listing_never_moves_it() {
    let emulator = Emulator::new();
    let seed = pair(&emulator, "settle-clock").await;
    let held = [
        connect(&emulator, &seed).await,
        connect(&emulator, &seed).await,
    ];

    // Before the span the create owes has passed every record is still PENDING. ONE read
    // here, as the sibling suites do (`device_lifecycle`, `link_lifecycle`), so the
    // assertion is not racing the span; that a read never MOVES the state is asserted
    // after it, where no clock bounds the loop.
    let (status, body) = get_connections(&emulator, &seed.network, "").await;
    assert_eq!(
        status, 200,
        "case the read before the settle span: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_in(&body),
        held,
        "case the read before the settle span: expected every record, got {body}"
    );
    assert_eq!(
        states_in(&body),
        vec!["PENDING".to_string(); held.len()],
        "case the read before the settle span: a fresh connection is PENDING, got {body}"
    );

    tokio::time::sleep(PAST_SETTLE).await;

    // Once the span is past the same records report AVAILABLE, however often they are
    // read: the transition is owed by the clock, and no number of reads moves the state
    // anywhere else (a listing that consumed or advanced anything would show here).
    let mut answers: Vec<Value> = Vec::new();
    for attempt in 0..3 {
        let (status, body) = get_connections(&emulator, &seed.network, "").await;
        assert_eq!(
            status, 200,
            "case read {attempt} after the settle span: expected 200, got {status} with {body}"
        );
        assert_eq!(
            states_in(&body),
            vec!["AVAILABLE".to_string(); held.len()],
            "case read {attempt} after the settle span: expected AVAILABLE, got {body}"
        );
        assert_eq!(
            ids_in(&body),
            held,
            "case read {attempt} after the settle span: expected every record, got {body}"
        );
        answers.push(body["Connections"].clone());
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "case repeated reads: every read must answer the same records, got {answers:?}"
    );

    // A filtered listing computes the same clock-derived state as the unfiltered one.
    let (status, body) = get_connections(
        &emulator,
        &seed.network,
        &format!("connectionIds={}", held[0]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a filtered read: expected 200, got {status} with {body}"
    );
    assert_eq!(
        states_in(&body),
        vec!["AVAILABLE".to_string()],
        "case a filtered read: expected the same clock-derived state, got {body}"
    );
}
