// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListAdapters` over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: Textract.ListAdapters`).
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * the listing reports every stored adapter, each as the four members
//!   `AdapterOverview` carries, with exactly the values `GetAdapter` reports for it;
//! * `AfterCreationTime`/`BeforeCreationTime` are the inclusive lower and upper bounds on
//!   an adapter's stored `CreationTime`, so an adapter is listed exactly when its creation
//!   time lies within the window the request gave;
//! * `MaxResults` cuts the listing into pages of at most that many adapters, and walking
//!   `NextToken` to the end yields the whole listing once, in one stable order (oldest
//!   adapter first), with no `NextToken` on the last page;
//! * a `NextToken` this listing never issued is refused with the modelled
//!   `InvalidParameterException`;
//! * the listing is a READ derived from the stored adapters: repeating it answers
//!   identically, and it follows `UpdateAdapter` and `DeleteAdapter`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;
use service_com_amazonaws_textract_textract::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator, with this one service mounted on a shared engine. The engine is handed
/// back too, so a test can seed adapters with the exact creation times it needs instead
/// of racing the clock.
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

/// `CreateAdapter`, asserting only that it succeeded, and the `AdapterId` it minted.
async fn create_adapter(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, body) = call(router, "CreateAdapter", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateAdapter, got {status} with {body}"
    );
    string_member(&body, "AdapterId")
}

/// `ListAdapters`, asserting only that it succeeded.
async fn list_adapters(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, body) = call(router, "ListAdapters", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListAdapters, got {status} with {body}"
    );
    body
}

/// The `Adapters` member of a listing, as a list of `AdapterOverview` objects.
fn overviews(body: &Value) -> Vec<Value> {
    body.get("Adapters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `AdapterId`s a listing reported, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    overviews(body)
        .iter()
        .map(|overview| string_member(overview, "AdapterId"))
        .collect()
}

/// An instant well inside the representable range, so seeded creation times and the
/// bounds compared against them are exact whole seconds on the wire.
const BASE_SECS: u64 = 1_700_000_000;

/// Store an adapter with an EXACT creation time, as `CreateAdapter` would have stored it.
///
/// `ListAdapters` filters on the stored `CreationTime`, which a create sets from the
/// clock; seeding lets a table name creation times that are seconds apart without the
/// test sleeping.
fn seed_adapter(
    engine: &Arc<engine::InMemoryEngine>,
    adapter_id: &str,
    adapter_name: &str,
    created_offset_secs: u64,
) {
    let service_state = state::service_state(&**engine).expect("the service state");
    service_state
        .lock()
        .insert_adapter(state::AdapterRecord::created(
            adapter_id.to_string(),
            state::AdapterConfiguration {
                adapter_name: adapter_name.to_string(),
                description: None,
                auto_update: state::DEFAULT_AUTO_UPDATE.to_string(),
            },
            vec![state::ADAPTER_FEATURE_TYPE.to_string()],
            HashMap::new(),
            UNIX_EPOCH + Duration::from_secs(BASE_SECS + created_offset_secs),
            None,
        ));
}

#[tokio::test]
async fn list_adapters_reports_every_stored_adapter_as_get_adapter_reports_it() {
    let (router, _engine) = harness();

    // Nothing created yet: the listing is empty and there is no further page.
    let empty = list_adapters(&router, "no adapters at all", &json!({})).await;
    assert_eq!(
        overviews(&empty),
        Vec::<Value>::new(),
        "with no adapters created the listing must be empty, got {empty}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "an empty listing has no further page, got {empty}"
    );

    // Rows differ along what the rule generalises over: how the adapter was created
    // (which optional members the request named, how many tags, how the feature type was
    // listed). Whatever the create said, the listing reports the stored adapter.
    let creates = [
        (
            "only the required members",
            json!({ "AdapterName": "minimal-adapter", "FeatureTypes": ["QUERIES"] }),
        ),
        (
            "a description and ENABLED auto update",
            json!({
                "AdapterName": "Claims_Adapter-02",
                "FeatureTypes": ["QUERIES"],
                "Description": "Trained on claim forms (2024): rev. 3!",
                "AutoUpdate": "ENABLED",
            }),
        ),
        (
            "three tags",
            json!({
                "AdapterName": "tagged-adapter",
                "FeatureTypes": ["QUERIES"],
                "Tags": { "team": "claims", "env": "test", "owner": "finance-ops" },
            }),
        ),
        (
            "the feature type listed twice",
            json!({
                "AdapterName": "repeated-feature-adapter",
                "FeatureTypes": ["QUERIES", "QUERIES"],
            }),
        ),
        (
            "a name at the modelled maximum length",
            json!({ "AdapterName": "a".repeat(128), "FeatureTypes": ["QUERIES"] }),
        ),
        (
            "a create carrying a ClientRequestToken",
            json!({
                "AdapterName": "tokened-adapter",
                "FeatureTypes": ["QUERIES"],
                "ClientRequestToken": "listing-token",
            }),
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, request) in &creates {
        created.push(create_adapter(&router, case, request).await);
    }

    let listing = list_adapters(&router, "every created adapter", &json!({})).await;
    assert_eq!(
        listed_ids(&listing).iter().collect::<BTreeSet<_>>(),
        created.iter().collect::<BTreeSet<_>>(),
        "the listing must report exactly the created adapters, got {listing}"
    );
    assert!(
        listing.get("NextToken").is_none(),
        "a listing that fits in one page has no NextToken, got {listing}"
    );

    // Each overview carries exactly `AdapterOverview`'s modelled members
    // (smithy_query(query="shape", shape="AdapterOverview"): AdapterId, AdapterName,
    // CreationTime, FeatureTypes) with the values `GetAdapter` reports for that adapter.
    let modelled_members: BTreeSet<&str> =
        ["AdapterId", "AdapterName", "CreationTime", "FeatureTypes"]
            .into_iter()
            .collect();
    for ((case, _), adapter_id) in creates.iter().zip(created.iter()) {
        let overview = overviews(&listing)
            .into_iter()
            .find(|overview| string_member(overview, "AdapterId") == *adapter_id)
            .unwrap_or_else(|| panic!("case {case:?}: {adapter_id:?} missing from {listing}"));
        let members: BTreeSet<&str> = overview
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            members, modelled_members,
            "case {case:?}: an AdapterOverview carries exactly the modelled members, got \
             {overview}"
        );

        let (status, read) = call(
            &router,
            "GetAdapter",
            &json!({ "AdapterId": adapter_id.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetAdapter of {adapter_id:?} must succeed, got {status} with {read}"
        );
        for member in &modelled_members {
            assert_eq!(
                overview.get(*member),
                read.get(*member),
                "case {case:?}: the listing's {member} must be what GetAdapter reports, got \
                 {overview} against {read}"
            );
        }
    }
}

/// One row of the creation-time window table.
struct WindowCase {
    name: &'static str,
    /// The `AfterCreationTime`/`BeforeCreationTime` members, as the request carries them.
    request: Value,
    /// The seeded adapters the window admits, by their creation offset in seconds.
    expect_offsets: Vec<u64>,
}

#[tokio::test]
async fn list_adapters_filters_on_the_inclusive_creation_time_window() {
    let (router, engine) = harness();

    // Four adapters, ten seconds apart, so a bound can fall ON one of them or between two.
    let offsets = [0u64, 10, 20, 30];
    for offset in offsets {
        seed_adapter(
            &engine,
            &format!("seeded-adapter-{offset:04}"),
            &format!("seeded-{offset}"),
            offset,
        );
    }
    let at = |offset: u64| json!(BASE_SECS + offset);

    // Rows differ along which side is bounded, whether a bound falls on an adapter's own
    // creation time or between two, and whether the window admits all, some or none.
    let cases = vec![
        WindowCase {
            name: "no bounds at all",
            request: json!({}),
            expect_offsets: vec![0, 10, 20, 30],
        },
        WindowCase {
            name: "a lower bound between two adapters",
            request: json!({ "AfterCreationTime": at(15) }),
            expect_offsets: vec![20, 30],
        },
        WindowCase {
            name: "a lower bound on an adapter's own creation time",
            request: json!({ "AfterCreationTime": at(10) }),
            expect_offsets: vec![10, 20, 30],
        },
        WindowCase {
            name: "an upper bound between two adapters",
            request: json!({ "BeforeCreationTime": at(15) }),
            expect_offsets: vec![0, 10],
        },
        WindowCase {
            name: "an upper bound on an adapter's own creation time",
            request: json!({ "BeforeCreationTime": at(20) }),
            expect_offsets: vec![0, 10, 20],
        },
        WindowCase {
            name: "both bounds, admitting the middle two",
            request: json!({ "AfterCreationTime": at(5), "BeforeCreationTime": at(25) }),
            expect_offsets: vec![10, 20],
        },
        WindowCase {
            name: "both bounds on one adapter's creation time",
            request: json!({ "AfterCreationTime": at(20), "BeforeCreationTime": at(20) }),
            expect_offsets: vec![20],
        },
        WindowCase {
            name: "a window entirely before every adapter",
            request: json!({ "BeforeCreationTime": at(0) }),
            expect_offsets: vec![0],
        },
        WindowCase {
            name: "a window entirely after every adapter",
            request: json!({ "AfterCreationTime": at(31) }),
            expect_offsets: vec![],
        },
        WindowCase {
            name: "a window that admits nothing between two adapters",
            request: json!({ "AfterCreationTime": at(11), "BeforeCreationTime": at(19) }),
            expect_offsets: vec![],
        },
        // A window whose bounds are the wrong way round is no range at all and is
        // REFUSED rather than listed; that refusal is asserted in
        // `crate::creation_time_windows`.
    ];

    for case in &cases {
        let listing = list_adapters(&router, case.name, &case.request).await;
        let expected: Vec<String> = case
            .expect_offsets
            .iter()
            .map(|offset| format!("seeded-adapter-{offset:04}"))
            .collect();
        assert_eq!(
            listed_ids(&listing),
            expected,
            "case {:?}: expected the adapters created at offsets {:?} ({expected:?}), got \
             {listing}",
            case.name,
            case.expect_offsets
        );
        // The listing's own `CreationTime` must agree with the window it was selected by.
        for overview in overviews(&listing) {
            let creation_time = overview
                .get("CreationTime")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            if let Some(after) = case.request.get("AfterCreationTime").and_then(Value::as_f64) {
                assert!(
                    creation_time >= after,
                    "case {:?}: {overview} is older than the AfterCreationTime bound {after}",
                    case.name
                );
            }
            if let Some(before) = case.request.get("BeforeCreationTime").and_then(Value::as_f64) {
                assert!(
                    creation_time <= before,
                    "case {:?}: {overview} is newer than the BeforeCreationTime bound {before}",
                    case.name
                );
            }
        }
    }
}

#[tokio::test]
async fn max_results_cuts_the_listing_into_pages_that_walk_the_whole_order() {
    let (router, engine) = harness();

    // Seven adapters with distinct creation times, seeded out of creation order so the
    // listing's order comes from the stored `CreationTime` and not from insertion.
    let seeded = [5u64, 0, 60, 30, 10, 45, 20];
    for offset in seeded {
        seed_adapter(
            &engine,
            &format!("seeded-adapter-{offset:04}"),
            &format!("seeded-{offset}"),
            offset,
        );
    }
    let mut oldest_first: Vec<u64> = seeded.to_vec();
    oldest_first.sort_unstable();
    let whole_listing: Vec<String> = oldest_first
        .iter()
        .map(|offset| format!("seeded-adapter-{offset:04}"))
        .collect();

    let unpaged = list_adapters(&router, "no MaxResults", &json!({})).await;
    assert_eq!(
        listed_ids(&unpaged),
        whole_listing,
        "the listing must report every adapter oldest first, got {unpaged}"
    );

    // Rows span page sizes below, at and above the number of adapters, including the
    // modelled minimum (`MaxResults` range 1..) and a size that does not divide the total.
    for max_results in [1usize, 2, 3, 6, 7, 8, 1000] {
        let case = format!("MaxResults = {max_results}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({ "MaxResults": max_results });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = list_adapters(&router, &case, &request).await;
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= max_results,
                "case {case:?}: a page holds at most MaxResults adapters, got {page}"
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= whole_listing.len() + 1,
                "case {case:?}: the walk must terminate, {pages} pages so far for {} adapters",
                whole_listing.len()
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => {
                    assert!(
                        walked.len() < whole_listing.len(),
                        "case {case:?}: a NextToken means adapters remain, got {page} after \
                         {walked:?}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, whole_listing,
            "case {case:?}: walking every page must yield the whole listing once, in order"
        );
        let expected_pages = whole_listing.len().div_ceil(max_results).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} adapters",
            whole_listing.len()
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let (router, engine) = harness();
    for offset in 0..4u64 {
        seed_adapter(
            &engine,
            &format!("seeded-adapter-{offset:04}"),
            &format!("seeded-{offset}"),
            offset,
        );
    }

    // A cursor is an offset into ONE listing, so it is accepted only for the listing it
    // was cut from. Rows span a forged token, a token of the wrong length, one of the
    // right length that is not a cursor, and a real cursor replayed under a DIFFERENT
    // creation-time window.
    let real = list_adapters(&router, "the first page", &json!({ "MaxResults": 1 })).await;
    let real_token = real
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a four-adapter listing paged at 1 has a NextToken")
        .to_string();
    let mut flipped = real_token.clone();
    flipped.pop();
    flipped.push(if real_token.ends_with('0') { '1' } else { '0' });

    let cases = vec![
        ("a token no listing issued", json!({ "NextToken": "not-a-cursor" })),
        (
            "a token of the issued length that is not a cursor",
            json!({ "NextToken": "0".repeat(real_token.len()) }),
        ),
        (
            "a real cursor with its last character changed",
            json!({ "NextToken": flipped }),
        ),
        (
            "a real cursor truncated",
            json!({ "NextToken": real_token[..real_token.len() - 1].to_string() }),
        ),
        (
            "a token at the modelled maximum PaginationToken length",
            json!({ "NextToken": "z".repeat(1024) }),
        ),
        (
            "a real cursor replayed under a different creation-time window",
            json!({ "NextToken": real_token.clone(), "AfterCreationTime": BASE_SECS }),
        ),
    ];

    for (case, request) in &cases {
        let (status, body) = call(&router, "ListAdapters", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        assert!(
            body.get("Adapters").is_none(),
            "case {case:?}: a refusal reports no adapters, got {body}"
        );
    }

    // The real cursor, under the window it was cut for, still works.
    let second = list_adapters(
        &router,
        "the real cursor",
        &json!({ "MaxResults": 1, "NextToken": real_token }),
    )
    .await;
    assert_eq!(
        listed_ids(&second),
        vec!["seeded-adapter-0001".to_string()],
        "the issued cursor must continue the listing it was cut from, got {second}"
    );
}

#[tokio::test]
async fn the_listing_is_a_read_that_follows_the_stored_adapters() {
    let (router, _engine) = harness();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..4u32 {
        ids.push(
            create_adapter(
                &router,
                &format!("adapter #{index}"),
                &json!({
                    "AdapterName": format!("listed-adapter-{index}"),
                    "FeatureTypes": ["QUERIES"],
                }),
            )
            .await,
        );
    }

    // A read: repeating it, with and without a page size, answers identically.
    for request in [json!({}), json!({ "MaxResults": 2 })] {
        let case = format!("repeated listing of {request}");
        let first = list_adapters(&router, &case, &request).await;
        for round in 0..3u32 {
            let again = list_adapters(&router, &case, &request).await;
            assert_eq!(
                again, first,
                "case {case:?}: round {round} answered {again} after {first}"
            );
        }
    }

    // And it is derived from the stored adapters, so it follows every mutation of them.
    let renamed = &ids[1];
    let (status, body) = call(
        &router,
        "UpdateAdapter",
        &json!({ "AdapterId": renamed, "AdapterName": "renamed-in-place" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateAdapter must succeed, got {status} with {body}"
    );
    let listing = list_adapters(&router, "after the rename", &json!({})).await;
    let names: HashMap<String, String> = overviews(&listing)
        .iter()
        .map(|overview| {
            (
                string_member(overview, "AdapterId"),
                string_member(overview, "AdapterName"),
            )
        })
        .collect();
    assert_eq!(
        names.get(renamed).map(String::as_str),
        Some("renamed-in-place"),
        "the listing must report the adapter's live name, got {listing}"
    );

    let deleted = ids[2].clone();
    let (status, body) = call(&router, "DeleteAdapter", &json!({ "AdapterId": deleted })).await;
    assert_eq!(
        status, 200,
        "DeleteAdapter must succeed, got {status} with {body}"
    );
    let listing = list_adapters(&router, "after the delete", &json!({})).await;
    let remaining: BTreeSet<String> = listed_ids(&listing).into_iter().collect();
    assert!(
        !remaining.contains(&deleted),
        "a deleted adapter must leave the listing, got {listing}"
    );
    assert_eq!(
        remaining,
        ids.iter()
            .filter(|id| **id != deleted)
            .cloned()
            .collect::<BTreeSet<String>>(),
        "every adapter that was not deleted must still be listed, got {listing}"
    );
}
