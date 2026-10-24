// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListImportFailures`.
//!
//! The rule under test: a `ListImportFailures` call reports the failures the import
//! its `ImportId` names in the caller's Region recorded, in the order they were
//! recorded, starting at the offset its `NextToken` names and carrying at most
//! `MaxResults` of them, and hands back a `NextToken` exactly while failures remain
//! past the page it returned. The operation models no not-found error, so an
//! `ImportId` naming no import of the caller's Region is answered with a page of no
//! failures — the same answer as an import that recorded none.
//!
//! The import registry these cases read is seeded through the shared engine, the
//! same store the handler reaches (`Emulator::state`): `StartImport` — the operation
//! that creates an import on AWS — is still a scaffold in this crate (its own stub
//! issue), so there is no wire call that fills the registry yet. Everything asserted
//! below is still read back over the wire, through the production router.

use serde_json::{json, Value};
use std::time::UNIX_EPOCH;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101 as cloudtrail;

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`. The import-registry fixtures these cases seed live beside
// it in `tests/all/harness.rs`.
use super::emulator::{Emulator, HOME_REGION};
use super::harness::{failure_time, import_id, seed_import, seeded_failure};

/// The `Location` of every failure a reply's `Failures` carries, in order.
fn locations(body: &Value) -> Vec<String> {
    body.get("Failures")
        .and_then(Value::as_array)
        .map(|failures| {
            failures
                .iter()
                .map(|failure| {
                    failure
                        .get("Location")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Page through every failure of `id`, `max_results` at a time, and hand back the
/// size of each page along with the locations the pages reported in order.
async fn page_through(
    emulator: &Emulator,
    case: &str,
    id: &str,
    max_results: Option<usize>,
) -> (Vec<usize>, Vec<String>) {
    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"ImportId": id});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let reply = emulator.call("ListImportFailures", &request).await;
        let body = reply.expect_ok(case).clone();
        let page = locations(&body);
        sizes.push(page.len());
        seen.extend(page);
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            sizes.len() < 16,
            "case {case}: the listing did not end after {} pages, sizes {sizes:?}",
            sizes.len()
        );
    }
    (sizes, seen)
}

/// The listing reports every recorded failure, in order, in pages of at most
/// `MaxResults`, with a token exactly while more remain.
#[tokio::test]
async fn list_import_failures_pages_through_the_failures_an_import_recorded() {
    let emulator = Emulator::new();
    let default_page = cloudtrail::state::LIST_IMPORT_FAILURES_DEFAULT_MAX_RESULTS;
    // (case, failures recorded, MaxResults, the page sizes the listing must take).
    // `MaxResults` is modelled `range 1..1000`, so the rows include each published
    // bound; `None` is the service's own default page size.
    let rows: Vec<(&str, usize, Option<usize>, Vec<usize>)> = vec![
        ("an import that recorded nothing", 0, None, vec![0]),
        ("fewer failures than one page", 3, None, vec![3]),
        ("exactly MaxResults failures", 4, Some(4), vec![4]),
        ("more failures than MaxResults", 7, Some(3), vec![3, 3, 1]),
        ("MaxResults at its lower bound", 3, Some(1), vec![1, 1, 1]),
        ("MaxResults at its upper bound", 5, Some(1000), vec![5]),
        (
            "more failures than the default page",
            default_page + 2,
            None,
            vec![default_page, 2],
        ),
    ];

    for (index, (case, recorded, max_results, expected_sizes)) in rows.into_iter().enumerate() {
        let id = seed_import(&emulator, HOME_REGION, index, recorded);
        let expected: Vec<String> = (0..recorded)
            .map(|position| seeded_failure(position).location)
            .collect();
        let (sizes, seen) = page_through(&emulator, case, &id, max_results).await;
        assert_eq!(
            sizes, expected_sizes,
            "case {case}: expected pages of {expected_sizes:?}, got {sizes:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case}: expected the {recorded} recorded failures in order, got {seen:?}"
        );
    }
}

/// Every member of a reported failure is the stored one, for each status the model's
/// `ImportFailureStatus` enum accepts.
#[tokio::test]
async fn a_reported_failure_carries_the_members_the_import_recorded() {
    let emulator = Emulator::new();
    let statuses = cloudtrail::ops::types::IMPORT_FAILURE_STATUS_VALUES;
    // One failure per modelled status, so every value in the catalog is reported.
    let id = seed_import(&emulator, HOME_REGION, 11, statuses.len());

    let reply = emulator
        .call("ListImportFailures", &json!({"ImportId": &id}))
        .await;
    let body = reply.expect_ok("a listing of one failure per status").clone();
    let failures = body
        .get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        failures.len(),
        statuses.len(),
        "case every status: expected {} failures, got {body:?}",
        statuses.len()
    );

    for (position, failure) in failures.iter().enumerate() {
        let expected = seeded_failure(position);
        let case = format!("status {}", expected.status);
        assert_eq!(
            (
                &failure["Location"],
                &failure["Status"],
                &failure["ErrorType"],
                &failure["ErrorMessage"]
            ),
            (
                &json!(expected.location),
                &json!(expected.status),
                &json!(expected.error_type),
                &json!(expected.error_message)
            ),
            "case {case}: expected the stored members, got {failure:?}"
        );
        let reported = failure["LastUpdatedTime"].as_f64();
        let stored = failure_time(position)
            .duration_since(UNIX_EPOCH)
            .expect("an instant after the epoch")
            .as_secs_f64();
        assert_eq!(
            reported,
            Some(stored),
            "case {case}: expected LastUpdatedTime {stored}, got {failure:?}"
        );
    }
}

/// An `ImportId` that names no import the caller's Region holds is a page of no
/// failures, and asking for one leaves the registry as it was.
#[tokio::test]
async fn an_import_the_caller_cannot_see_has_no_failures() {
    let emulator = Emulator::new();
    let elsewhere = seed_import(&emulator, "eu-west-1", 21, 4);
    let here = seed_import(&emulator, HOME_REGION, 22, 2);

    // (case, the id to ask about, the Region to ask from)
    let rows = [
        ("an unknown import id", import_id(99), HOME_REGION),
        ("an import of another Region", elsewhere.clone(), HOME_REGION),
        ("a known import from another Region", here.clone(), "eu-west-1"),
    ];
    for (case, id, region) in rows {
        let reply = emulator
            .call_in("ListImportFailures", region, &json!({"ImportId": id}))
            .await;
        let body = reply.expect_ok(case).clone();
        assert_eq!(
            (body.get("Failures"), body.get("NextToken")),
            (Some(&json!([])), None),
            "case {case}: expected an empty page and no token, got {body:?}"
        );
    }

    // The listing is a read: the imports it could not see are untouched, and the
    // failures of the ones it could are still there.
    let store = emulator.state();
    let state = store.lock();
    assert_eq!(
        (
            state.imports.len(),
            state
                .import_failures(&elsewhere, "eu-west-1")
                .len(),
            state.import_failures(&here, HOME_REGION).len(),
        ),
        (2, 4, 2),
        "listing failures must not change the registry, got {:?} / {:?}",
        state.imports,
        state.import_failures
    );
}

/// A `NextToken` belongs to the request that minted it: it resumes that import's
/// listing, and any other token is refused.
#[tokio::test]
async fn a_pagination_token_is_valid_only_for_the_import_it_was_minted_for() {
    let emulator = Emulator::new();
    let id = seed_import(&emulator, HOME_REGION, 31, 5);
    let other = seed_import(&emulator, HOME_REGION, 32, 5);

    // The positive control: the token the first page hands out resumes the listing.
    let first = emulator
        .call(
            "ListImportFailures",
            &json!({"ImportId": &id, "MaxResults": 2}),
        )
        .await;
    let first = first.expect_ok("the first page").clone();
    let token = first["NextToken"]
        .as_str()
        .expect("a token while failures remain")
        .to_string();
    let resumed = emulator
        .call(
            "ListImportFailures",
            &json!({"ImportId": &id, "MaxResults": 2, "NextToken": &token}),
        )
        .await;
    let resumed = resumed.expect_ok("the page after the token").clone();
    assert_eq!(
        locations(&resumed),
        vec![seeded_failure(2).location, seeded_failure(3).location],
        "case resumed page: expected the failures after the first page, got {resumed:?}"
    );

    // A token past the end of the list is the end of the listing, not a failure.
    let past_the_end = emulator
        .call(
            "ListImportFailures",
            &json!({"ImportId": &id, "NextToken": format!("{id}/9")}),
        )
        .await;
    let past_the_end = past_the_end.expect_ok("a token past the end").clone();
    assert_eq!(
        (past_the_end.get("Failures"), past_the_end.get("NextToken")),
        (Some(&json!([])), None),
        "case past the end: expected an empty final page, got {past_the_end:?}"
    );

    // (case, the token presented with `id`) — each is invalid for this request.
    // `NextToken` is modelled len 4..1000, so every row is long enough to reach the
    // handler rather than being refused as a constraint violation.
    let rows = [
        ("a token that is not one of ours", "abcd".to_string()),
        ("a token minted for another import", format!("{other}/2")),
        ("a token with no offset", format!("{id}/")),
        ("a token with a non-numeric offset", format!("{id}/second")),
        ("a token with a negative offset", format!("{id}/-1")),
    ];
    for (case, token) in rows {
        emulator
            .call(
                "ListImportFailures",
                &json!({"ImportId": &id, "NextToken": token}),
            )
            .await
            .expect_error(case, 400, "InvalidNextTokenException");
    }
}
