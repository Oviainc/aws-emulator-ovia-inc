// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAdapterVersions` over the wire (`awsJson1_1`, `POST /`
//! with `X-Amz-Target: Textract.ListAdapterVersions`).
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * the listing reports the adapter versions this service HOLDS, scoped to one adapter
//!   when `AdapterId` names one and across every adapter when it does not — so an
//!   adapter with no versions lists an empty `AdapterVersions`, whatever its siblings
//!   hold;
//! * every reported member comes from the stored version, and `Status` is derived from
//!   the clock: a version whose creation is not yet due reports
//!   `CREATION_IN_PROGRESS`, and one that has settled reports the status it settled
//!   into, with its `StatusMessage`;
//! * `AfterCreationTime` and `BeforeCreationTime` are the window's inclusive lower and
//!   upper bounds;
//! * `MaxResults` cuts the window into pages and `NextToken` walks them, every version
//!   exactly once, with no `NextToken` on the last page; a token is bound to the query
//!   that issued it;
//! * an `AdapterId` that names no adapter is refused with the modelled
//!   `ResourceNotFoundException`;
//! * the listing is a READ: repeating it answers identically.
//!
//! `CreateAdapterVersion` is still a scaffold (its own stub issue), so these tests plant
//! versions in the service's store — the one place a created version will live — and
//! then read them back over the wire, which is exactly what `ListAdapterVersions` has to
//! derive its answer from.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::ops::types::ADAPTER_VERSION_STATUS_VALUES;
use service_com_amazonaws_textract_textract::state;
use service_com_amazonaws_textract_textract::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator, with this one service mounted on a shared engine, plus that engine —
/// the store a created adapter version lives in.
fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Textract.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn string_member(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CreateAdapter` with `name`, answering the `AdapterId` it minted.
async fn create_adapter(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateAdapter",
        &json!({ "AdapterName": name, "FeatureTypes": ["QUERIES"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAdapter, got {status} with {body}"
    );
    string_member(&body, "AdapterId")
}

/// `ListAdapterVersions`, asserting only that it succeeded.
async fn list(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, body) = call(router, "ListAdapterVersions", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListAdapterVersions, got {status} with {body}"
    );
    body
}

/// The `AdapterVersion` of each entry in a listing, in the order it reported them.
fn reported_versions(body: &Value) -> Vec<String> {
    body.get("AdapterVersions")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| string_member(entry, "AdapterVersion"))
                .collect()
        })
        .unwrap_or_default()
}

/// An instant `offset` seconds after a fixed, whole-second epoch instant.
///
/// Whole seconds so a `CreationTime` compared on the wire is exact: the awsJson
/// protocols carry a timestamp as epoch seconds.
fn at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset)
}

/// Epoch seconds of `instant`, as a `CreationTime` comes back on the wire.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// One adapter version to plant in the store.
struct Seed {
    adapter_version: String,
    created_at: SystemTime,
    /// When the `CREATION_IN_PROGRESS` -> `settled_status` transition becomes due.
    settles_at: SystemTime,
    settled_status: String,
    settled_status_message: Option<String>,
}

impl Seed {
    /// A version that has already settled into `ACTIVE`, created at `at(offset)`.
    fn active(adapter_version: &str, offset: u64) -> Self {
        Self {
            adapter_version: adapter_version.to_string(),
            created_at: at(offset),
            settles_at: at(offset),
            settled_status: "ACTIVE".to_string(),
            settled_status_message: None,
        }
    }
}

/// Plant `seed` on `adapter_id` in the service's store — where `CreateAdapterVersion`
/// will put a created version, and the only thing `ListAdapterVersions` reads.
fn seed_version(engine: &engine::InMemoryEngine, adapter_id: &str, seed: &Seed) {
    let store = state::service_state(engine).expect("the textract store");
    let mut store = store.lock();
    let adapter = store
        .adapters
        .get_mut(adapter_id)
        .expect("the adapter the version belongs to");
    let feature_types = adapter.feature_types.clone();
    adapter.versions.insert(
        seed.adapter_version.clone(),
        state::AdapterVersionRecord {
            adapter_id: adapter_id.to_string(),
            adapter_version: seed.adapter_version.clone(),
            created_at: seed.created_at,
            feature_types,
            settles_at: seed.settles_at,
            settled_status: seed.settled_status.clone(),
            settled_status_message: seed.settled_status_message.clone(),
        },
    );
}

#[tokio::test]
async fn an_adapter_with_no_versions_lists_an_empty_version_list() {
    let (router, engine) = harness();
    let bare = create_adapter(&router, "the adapter under test", "bare-adapter").await;
    // A SIBLING adapter that does hold versions, so an empty answer cannot come from an
    // empty store.
    let stocked = create_adapter(&router, "the sibling", "stocked-adapter").await;
    for (index, version) in ["1", "2", "3"].iter().enumerate() {
        seed_version(&engine, &stocked, &Seed::active(version, index as u64 * 10));
    }

    // The rows differ along which filtration criteria the request names: the rule is
    // that an adapter holding no version lists none, whatever else the request asks for.
    let cases = [
        ("no criteria at all", json!({ "AdapterId": bare.clone() })),
        (
            "a page size",
            json!({ "AdapterId": bare.clone(), "MaxResults": 10 }),
        ),
        (
            "a creation-time window",
            json!({
                "AdapterId": bare.clone(),
                "AfterCreationTime": epoch_seconds(at(0)),
                "BeforeCreationTime": epoch_seconds(at(1_000)),
            }),
        ),
    ];

    for (name, request) in &cases {
        let body = list(&router, name, request).await;
        assert_eq!(
            body.get("AdapterVersions").cloned().unwrap_or(Value::Null),
            json!([]),
            "case {name:?}: an adapter with no versions must report an empty AdapterVersions, \
             got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {name:?}: an empty listing has no further page, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_every_member_of_the_stored_version() {
    let (router, engine) = harness();
    let adapter = create_adapter(&router, "the adapter under test", "reported-adapter").await;

    // The rows differ along what the rule generalises over: how long the version string
    // is (`AdapterVersion` is modelled len 1..128), when it was created, which settled
    // status it carries, and whether it carries a StatusMessage.
    let seeds = [
        Seed {
            adapter_version: "1".to_string(),
            created_at: at(0),
            settles_at: at(0),
            settled_status: "ACTIVE".to_string(),
            settled_status_message: None,
        },
        Seed {
            adapter_version: "2".to_string(),
            created_at: at(60),
            settles_at: at(60),
            settled_status: "AT_RISK".to_string(),
            settled_status_message: Some("Adapter version performance is degraded".to_string()),
        },
        Seed {
            // `AdapterVersion` is modelled len 1..128: this row sits on the maximum.
            adapter_version: "v".repeat(128),
            created_at: at(120),
            settles_at: at(120),
            settled_status: "DEPRECATED".to_string(),
            settled_status_message: Some("Adapter version is deprecated".to_string()),
        },
    ];
    for seed in &seeds {
        seed_version(&engine, &adapter, seed);
    }

    let body = list(&router, "every member", &json!({ "AdapterId": adapter.clone() })).await;
    let entries = body
        .get("AdapterVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        seeds.len(),
        "expected {} versions, got {body}",
        seeds.len()
    );

    for (seed, entry) in seeds.iter().zip(entries.iter()) {
        let case = seed.adapter_version.as_str();
        assert_eq!(
            string_member(entry, "AdapterId"),
            adapter,
            "case {case:?}: expected AdapterId {adapter:?}, got {entry}"
        );
        assert_eq!(
            string_member(entry, "AdapterVersion"),
            seed.adapter_version,
            "case {case:?}: expected AdapterVersion {:?}, got {entry}",
            seed.adapter_version
        );
        assert_eq!(
            entry.get("CreationTime").and_then(Value::as_f64),
            Some(epoch_seconds(seed.created_at)),
            "case {case:?}: expected CreationTime {}, got {entry}",
            epoch_seconds(seed.created_at)
        );
        assert_eq!(
            entry.get("FeatureTypes").cloned().unwrap_or(Value::Null),
            json!(["QUERIES"]),
            "case {case:?}: a version operates on its adapter's feature types, got {entry}"
        );
        assert_eq!(
            string_member(entry, "Status"),
            seed.settled_status,
            "case {case:?}: expected Status {:?}, got {entry}",
            seed.settled_status
        );
        match seed.settled_status_message.as_deref() {
            Some(expected) => assert_eq!(
                string_member(entry, "StatusMessage"),
                expected,
                "case {case:?}: expected StatusMessage {expected:?}, got {entry}"
            ),
            None => assert!(
                entry.get("StatusMessage").is_none(),
                "case {case:?}: a version with no status message must report none, got {entry}"
            ),
        }
    }
    assert!(
        body.get("NextToken").is_none(),
        "the last page carries no NextToken, got {body}"
    );
}

#[tokio::test]
async fn status_is_derived_from_whether_the_creation_is_due() {
    let (router, engine) = harness();
    let adapter = create_adapter(&router, "the adapter under test", "status-adapter").await;

    // The catalog is the modelled `AdapterVersionStatus` enum
    // (ADAPTER_VERSION_STATUS_VALUES, from this service's Smithy model): every value a
    // version can settle into is exercised, so a value added to the model shows up here
    // rather than going untested.
    let settled: Vec<&str> = ADAPTER_VERSION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| *status != state::ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS)
        .collect();
    assert!(
        ADAPTER_VERSION_STATUS_VALUES
            .contains(&state::ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS),
        "the status reported while a creation is not due must be one the model declares, \
         got {:?} outside {ADAPTER_VERSION_STATUS_VALUES:?}",
        state::ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS
    );
    assert!(
        settled.len() > 1,
        "the model must declare statuses a version can settle into, got {ADAPTER_VERSION_STATUS_VALUES:?}"
    );

    // One version per settled status whose transition is already due, and one per status
    // whose transition is NOT due yet. The due instant is the only difference between
    // the two rows of a pair.
    let far_future = SystemTime::now() + Duration::from_secs(3_600);
    let message = "Adapter version reached its settled status";
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (index, status) in settled.iter().enumerate() {
        let offset = index as u64 * 10;
        let due = format!("due-{status}");
        let pending = format!("pending-{status}");
        seed_version(
            &engine,
            &adapter,
            &Seed {
                adapter_version: due.clone(),
                created_at: at(offset),
                settles_at: at(offset),
                settled_status: (*status).to_string(),
                settled_status_message: Some(message.to_string()),
            },
        );
        seed_version(
            &engine,
            &adapter,
            &Seed {
                adapter_version: pending.clone(),
                created_at: at(offset + 1),
                settles_at: far_future,
                settled_status: (*status).to_string(),
                settled_status_message: Some(message.to_string()),
            },
        );
        expected.insert(due, (*status).to_string());
        expected.insert(
            pending,
            state::ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS.to_string(),
        );
    }

    let body = list(
        &router,
        "clock-derived status",
        &json!({ "AdapterId": adapter.clone(), "MaxResults": 100 }),
    )
    .await;
    let entries = body
        .get("AdapterVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        expected.len(),
        "expected {} versions, got {body}",
        expected.len()
    );

    for entry in &entries {
        let version = string_member(entry, "AdapterVersion");
        let wanted = expected
            .get(&version)
            .unwrap_or_else(|| panic!("case {version:?}: unexpected version in {body}"));
        assert_eq!(
            &string_member(entry, "Status"),
            wanted,
            "case {version:?}: expected Status {wanted:?}, got {entry}"
        );
        if wanted == state::ADAPTER_VERSION_STATUS_CREATION_IN_PROGRESS {
            assert!(
                entry.get("StatusMessage").is_none(),
                "case {version:?}: a version whose creation is not due carries no settled \
                 StatusMessage, got {entry}"
            );
        } else {
            assert_eq!(
                string_member(entry, "StatusMessage"),
                message,
                "case {version:?}: expected the settled status message, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn creation_time_bounds_are_the_windows_inclusive_edges() {
    let (router, engine) = harness();
    let adapter = create_adapter(&router, "the adapter under test", "window-adapter").await;
    // Four versions, 100 seconds apart, so a bound can fall ON one of them or between
    // two.
    for (version, offset) in [("1", 0u64), ("2", 100), ("3", 200), ("4", 300)] {
        seed_version(&engine, &adapter, &Seed::active(version, offset));
    }

    // The rows differ along where each bound falls: absent, exactly on a creation
    // instant (the bound itself is included), between two, and outside the whole set.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no bounds at all", json!({}), vec!["1", "2", "3", "4"]),
        (
            "AfterCreationTime on the second version's creation instant",
            json!({ "AfterCreationTime": epoch_seconds(at(100)) }),
            vec!["2", "3", "4"],
        ),
        (
            "AfterCreationTime one second past a creation instant",
            json!({ "AfterCreationTime": epoch_seconds(at(101)) }),
            vec!["3", "4"],
        ),
        (
            "BeforeCreationTime on the third version's creation instant",
            json!({ "BeforeCreationTime": epoch_seconds(at(200)) }),
            vec!["1", "2", "3"],
        ),
        (
            "BeforeCreationTime one second before a creation instant",
            json!({ "BeforeCreationTime": epoch_seconds(at(199)) }),
            vec!["1", "2"],
        ),
        (
            "both bounds, spanning the middle two",
            json!({
                "AfterCreationTime": epoch_seconds(at(100)),
                "BeforeCreationTime": epoch_seconds(at(200)),
            }),
            vec!["2", "3"],
        ),
        (
            "both bounds on one creation instant",
            json!({
                "AfterCreationTime": epoch_seconds(at(300)),
                "BeforeCreationTime": epoch_seconds(at(300)),
            }),
            vec!["4"],
        ),
        (
            "a window after every version",
            json!({ "AfterCreationTime": epoch_seconds(at(1_000)) }),
            vec![],
        ),
        (
            "a window before every version",
            json!({ "BeforeCreationTime": epoch_seconds(at(0) - Duration::from_secs(1)) }),
            vec![],
        ),
        // A window whose bounds are crossed is no range at all and is REFUSED rather
        // than listed; that refusal is asserted in `crate::creation_time_windows`.
    ];

    for (name, criteria, wanted) in &cases {
        let mut request = json!({ "AdapterId": adapter.clone() });
        for (key, value) in criteria.as_object().expect("an object of criteria") {
            request[key] = value.clone();
        }
        let body = list(&router, name, &request).await;
        assert_eq!(
            reported_versions(&body),
            wanted.iter().map(|v| (*v).to_string()).collect::<Vec<_>>(),
            "case {name:?}: expected versions {wanted:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn max_results_pages_the_listing_and_next_token_walks_it() {
    let (router, engine) = harness();
    let adapter = create_adapter(&router, "the adapter under test", "paged-adapter").await;
    let versions: Vec<&'static str> = vec!["1", "2", "3", "4", "5", "6", "7"];
    for (index, version) in versions.iter().enumerate() {
        seed_version(&engine, &adapter, &Seed::active(version, index as u64 * 10));
    }
    let all: Vec<String> = versions.iter().map(|v| (*v).to_string()).collect();

    // The rows differ along the page size: one per page, a size that divides the set,
    // one that does not, exactly the set, and more than the set. Whatever the size, the
    // walk must report every version exactly once in the same order.
    for page_size in [1i64, 2, 3, 6, 7, 8] {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "AdapterId": adapter.clone(), "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let body = list(&router, &case, &request).await;
            let page = reported_versions(&body);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page must hold at most MaxResults versions, got {body}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: the walk must terminate, saw {pages} pages"
            );
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: the walk must report every version exactly once in order, got {seen:?}"
        );
    }

    // The page size may change between pages: the cursor names a position in the result
    // set, not a page number.
    let first = list(
        &router,
        "changing page size",
        &json!({ "AdapterId": adapter.clone(), "MaxResults": 2 }),
    )
    .await;
    let token = string_member(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "a listing cut short must carry a NextToken, got {first}"
    );
    let second = list(
        &router,
        "changing page size",
        &json!({ "AdapterId": adapter.clone(), "MaxResults": 100, "NextToken": token }),
    )
    .await;
    let mut walked = reported_versions(&first);
    walked.extend(reported_versions(&second));
    assert_eq!(
        walked, all,
        "case \"changing page size\": the two pages must cover the set once, got {walked:?}"
    );
}

#[tokio::test]
async fn a_next_token_not_issued_for_this_listing_is_refused() {
    let (router, engine) = harness();
    let first = create_adapter(&router, "the first adapter", "token-adapter-one").await;
    let second = create_adapter(&router, "the second adapter", "token-adapter-two").await;
    for (index, version) in ["1", "2", "3"].iter().enumerate() {
        seed_version(&engine, &first, &Seed::active(version, index as u64 * 10));
        seed_version(&engine, &second, &Seed::active(version, index as u64 * 10));
    }

    let issued = list(
        &router,
        "the listing that issued the token",
        &json!({ "AdapterId": first.clone(), "MaxResults": 1 }),
    )
    .await;
    let token = string_member(&issued, "NextToken");
    assert!(
        !token.is_empty(),
        "the listing must issue a NextToken, got {issued}"
    );

    // The rows differ along how the replay differs from the listing that issued the
    // token: another adapter, another creation-time window, a token nobody issued, and a
    // token of the wrong shape.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the token replayed against another adapter",
            json!({ "AdapterId": second.clone(), "NextToken": token.clone() }),
        ),
        (
            "the token replayed with a creation-time window",
            json!({
                "AdapterId": first.clone(),
                "AfterCreationTime": epoch_seconds(at(0)),
                "NextToken": token.clone(),
            }),
        ),
        (
            "the token replayed across every adapter",
            json!({ "NextToken": token.clone() }),
        ),
        (
            "a token of the issued length nobody issued",
            json!({ "AdapterId": first.clone(), "NextToken": "0".repeat(token.len()) }),
        ),
        (
            "a token that is not a cursor at all",
            json!({ "AdapterId": first.clone(), "NextToken": "not-a-cursor" }),
        ),
        (
            "a token holding a non-ASCII character",
            json!({ "AdapterId": first.clone(), "NextToken": "é".repeat(16) }),
        ),
    ];

    for (name, request) in &cases {
        let (status, body) = call(&router, "ListAdapterVersions", request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {name:?}: expected InvalidParameterException, got {body}"
        );
        assert!(
            body.get("AdapterVersions").is_none(),
            "case {name:?}: a refusal reports no versions, got {body}"
        );
    }
}

#[tokio::test]
async fn an_adapter_id_no_create_minted_is_refused() {
    let (router, engine) = harness();
    let known = create_adapter(&router, "the one real adapter", "real-adapter").await;
    seed_version(&engine, &known, &Seed::active("1", 0));

    // Rows span the modelled AdapterId length (12..1011, from smithy_query
    // shape=AdapterId) and an id that differs from a real one by a single character.
    let mut flipped = known.clone();
    flipped.pop();
    flipped.push(if known.ends_with('0') { '1' } else { '0' });
    let cases = [
        ("an id at the modelled minimum length", "a".repeat(12)),
        ("an id at the modelled maximum length", "b".repeat(1011)),
        ("a plausible 32-character id", "c".repeat(32)),
        ("a real id with its last character changed", flipped),
    ];

    for (name, adapter_id) in &cases {
        assert_ne!(
            adapter_id, &known,
            "case {name:?}: the table must not name the adapter that exists"
        );
        let (status, body) = call(
            &router,
            "ListAdapterVersions",
            &json!({ "AdapterId": adapter_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            body.get("AdapterVersions").is_none(),
            "case {name:?}: a refusal reports no versions, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listing_with_no_adapter_id_spans_every_adapter() {
    let (router, engine) = harness();
    // Two adapters, each with its own versions: an unscoped listing reports both, and a
    // scoped one reports only its own.
    let mut owners: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for (index, (name, versions)) in [
        ("first-adapter", vec!["1", "2"]),
        ("second-adapter", vec!["1", "2", "3"]),
    ]
    .into_iter()
    .enumerate()
    {
        let adapter_id = create_adapter(&router, name, name).await;
        for (slot, version) in versions.iter().enumerate() {
            let offset = (index * 100 + slot * 10) as u64;
            seed_version(&engine, &adapter_id, &Seed::active(version, offset));
        }
        owners.insert(adapter_id, versions);
    }

    let unscoped = list(&router, "every adapter", &json!({ "MaxResults": 100 })).await;
    let entries = unscoped
        .get("AdapterVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total: usize = owners.values().map(Vec::len).sum();
    assert_eq!(
        entries.len(),
        total,
        "an unscoped listing must report every stored version, expected {total}, got {unscoped}"
    );
    let reported_owners: BTreeSet<String> = entries
        .iter()
        .map(|entry| string_member(entry, "AdapterId"))
        .collect();
    assert_eq!(
        reported_owners,
        owners.keys().cloned().collect::<BTreeSet<String>>(),
        "every version must report the adapter it belongs to, got {unscoped}"
    );

    for (adapter_id, versions) in &owners {
        let body = list(&router, adapter_id, &json!({ "AdapterId": adapter_id })).await;
        assert_eq!(
            reported_versions(&body),
            versions.iter().map(|v| (*v).to_string()).collect::<Vec<_>>(),
            "case {adapter_id:?}: a scoped listing reports only that adapter's versions, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = harness();
    let adapter = create_adapter(&router, "the adapter under test", "stable-adapter").await;
    for (index, version) in ["1", "2", "3"].iter().enumerate() {
        seed_version(&engine, &adapter, &Seed::active(version, index as u64 * 10));
    }
    let request = json!({ "AdapterId": adapter.clone() });

    let first = list(&router, "the first read", &request).await;
    // Reading again, paging through, and creating other adapters in between must not
    // move anything.
    for index in 0..3u32 {
        let case = format!("after {index} intervening calls");
        let repeat = list(&router, &case, &request).await;
        assert_eq!(
            repeat, first,
            "case {case:?}: a read must answer identically, got {repeat} after {first}"
        );
        list(
            &router,
            &case,
            &json!({ "AdapterId": adapter.clone(), "MaxResults": 1 }),
        )
        .await;
        create_adapter(&router, &case, &format!("other-adapter-{index}")).await;
    }
}
