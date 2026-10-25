// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListIdentityPools`, the account-wide listing of identity pools.
//!
//! The RULES every case asserts, over a table of inputs rather than one recorded call:
//!
//! * A page reports the pools this service holds — each with the id and the name
//!   `CreateIdentityPool` stored — in one total order that is the same from one call to
//!   the next, so successive pages partition the stored set: no pool twice, none missed.
//!   A page carries at most `MaxResults` pools and its `NextToken` resumes exactly where
//!   it ended, absent on the last page.
//! * The listing reflects the store: a pool appears as soon as it is created and is gone
//!   as soon as it is deleted, whatever its name — two pools that share a name, or a
//!   developer provider name, are two pools and both are listed.
//! * Listing is a read: it leaves the store exactly as it found it.
//! * A `NextToken` this listing did not issue, including one whose pool has since been
//!   deleted, is refused with the modelled 400 `InvalidParameterException`, while a
//!   `MaxResults` outside the modelled range never reaches the operation: the
//!   constraint-validation layer ahead of it refuses the request with 400
//!   `ValidationException`, naming the member and the constraint the value failed. Every
//!   value inside the range, each bound included, is served.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::identity_pool_flow::{error_code, Emulator};

/// `MaxResults` is modelled `QueryLimit, range 1..60`
/// (`smithy_query(query="operation", operation="ListIdentityPools")`), so these are the
/// bounds of what the service serves and the first value past each of them.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 60;

/// One pool as a page reports it: its id and its name.
type ListedPool = (String, String);

/// Every pool a listing reports, read page by page with `page_size` per page. Asserts the
/// per-page invariants on the way through — a page never exceeds `page_size`, a page that
/// carries a `NextToken` is not empty, and the walk terminates — and returns the pools in
/// the order the pages reported them.
async fn page_through(
    emulator: &Emulator,
    case: &str,
    page_size: i64,
    stored: usize,
) -> Vec<ListedPool> {
    let mut listed: Vec<ListedPool> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = Map::new();
        request.insert("MaxResults".to_string(), json!(page_size));
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), json!(token));
        }
        let page = emulator
            .ok("ListIdentityPools", &Value::Object(request))
            .await;
        pages += 1;
        assert!(
            pages <= stored + 1,
            "case {case:?}: paging {stored} pools {page_size} at a time did not terminate \
             after {pages} pages, last page {page}"
        );
        let pools = page["IdentityPools"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a page carries an IdentityPools list: {page}"))
            .clone();
        assert!(
            i64::try_from(pools.len()).unwrap_or(i64::MAX) <= page_size,
            "case {case:?}: expected at most {page_size} pools on a page, got {} in {page}",
            pools.len()
        );
        for pool in &pools {
            let identity_pool_id = pool["IdentityPoolId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed pool carries an IdentityPoolId: {pool}"))
                .to_string();
            let identity_pool_name = pool["IdentityPoolName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a listed pool carries an IdentityPoolName: {pool}"))
                .to_string();
            listed.push((identity_pool_id, identity_pool_name));
        }
        next_token = page["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            !pools.is_empty(),
            "case {case:?}: a page carrying a NextToken must not be empty: {page}"
        );
    }
    listed
}

/// RULE: a listing reports every stored pool exactly once, with the id and name
/// `CreateIdentityPool` stored, in one order that does not depend on the page size or on
/// how many times the listing is read.
#[tokio::test]
async fn a_listing_reports_every_stored_pool_exactly_once() {
    let emulator = Emulator::new();

    // The pools: one row per interesting NAME, since the name is what a page reports
    // beside the id — two pools sharing a name, a name at the modelled 1-character
    // minimum, and names whose sort order differs from their creation order, so a stable
    // order is asserted over inputs that cannot all be ordered by accident.
    let names = [
        "zulu-pool",
        "alpha-pool",
        "a",
        "shared-name",
        "shared-name",
        "mid-pool",
        "Pool_With.Mixed-Case",
    ];
    let mut expected: Vec<ListedPool> = Vec::new();
    for name in names {
        let identity_pool_id = emulator.pool(name, true).await;
        expected.push((identity_pool_id, name.to_string()));
    }
    let expected: BTreeSet<ListedPool> = expected.into_iter().collect();

    // Every page size: both bounds of the modelled range, and sizes that divide the pool
    // count evenly, unevenly and not at all.
    let page_sizes = [MAX_RESULTS_MIN, 2, 3, 7, 8, MAX_RESULTS_MAX];
    let mut orders: Vec<Vec<ListedPool>> = Vec::new();
    for page_size in page_sizes {
        let case = format!("page size {page_size}");
        let listed = page_through(&emulator, &case, page_size, expected.len()).await;
        let as_set: BTreeSet<ListedPool> = listed.iter().cloned().collect();
        assert_eq!(
            as_set.len(),
            listed.len(),
            "case {case:?}: a pool was reported twice across the pages: {listed:?}"
        );
        assert_eq!(
            as_set, expected,
            "case {case:?}: expected the stored pools {expected:?}, got {as_set:?}"
        );
        orders.push(listed);
    }
    for (page_size, order) in page_sizes.iter().zip(&orders) {
        assert_eq!(
            order, &orders[0],
            "case \"page size {page_size}\": expected the order of {:?}, got {order:?}",
            orders[0]
        );
    }
}

/// RULE: the listing reflects the store — a pool shows up as soon as it is created and is
/// gone as soon as it is deleted — and reading it changes nothing a later caller sees.
#[tokio::test]
async fn the_listing_reflects_what_is_stored() {
    let emulator = Emulator::new();

    // An account with no pools lists none, and says so with an empty list rather than a
    // page that resumes somewhere.
    let empty = emulator
        .ok("ListIdentityPools", &json!({"MaxResults": MAX_RESULTS_MAX}))
        .await;
    assert_eq!(
        empty["IdentityPools"].as_array().map(Vec::len),
        Some(0),
        "an account holding no pools lists none, got {empty}"
    );
    assert!(
        empty["NextToken"].is_null(),
        "a listing with nothing left to report carries no NextToken, got {empty}"
    );

    // One row per shape of pool a create can take, including two that share a
    // `DeveloperProviderName`: that name is not a key of the pool, so both are stored and
    // both are listed.
    let creates = [
        json!({"IdentityPoolName": "plain", "AllowUnauthenticatedIdentities": true}),
        json!({"IdentityPoolName": "closed", "AllowUnauthenticatedIdentities": false}),
        json!({
            "IdentityPoolName": "dev-one",
            "AllowUnauthenticatedIdentities": true,
            "DeveloperProviderName": "login.example.shared",
        }),
        json!({
            "IdentityPoolName": "dev-two",
            "AllowUnauthenticatedIdentities": true,
            "DeveloperProviderName": "login.example.shared",
        }),
    ];
    let mut expected: BTreeSet<ListedPool> = BTreeSet::new();
    for create in &creates {
        let created = emulator.ok("CreateIdentityPool", create).await;
        let identity_pool_id = created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("create {create} returns an IdentityPoolId: {created}"))
            .to_string();
        let identity_pool_name = created["IdentityPoolName"]
            .as_str()
            .unwrap_or_else(|| panic!("create {create} returns an IdentityPoolName: {created}"))
            .to_string();
        expected.insert((identity_pool_id, identity_pool_name));
        let case = format!("after creating {create}");
        let listed: BTreeSet<ListedPool> = page_through(&emulator, &case, 2, expected.len())
            .await
            .into_iter()
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the pools created so far {expected:?}, got {listed:?}"
        );
    }

    // Reading the listing twice over leaves it reporting the same thing: it is a read.
    let reread: BTreeSet<ListedPool> = page_through(&emulator, "re-read", 1, expected.len())
        .await
        .into_iter()
        .collect();
    assert_eq!(
        reread, expected,
        "case \"re-read\": a listing is a read, so a second one reports the same pools \
         {expected:?}, got {reread:?}"
    );

    // And a deleted pool leaves the listing, while every other pool stays in it.
    while let Some(doomed) = expected.first().cloned() {
        expected.remove(&doomed);
        emulator
            .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &doomed.0}))
            .await;
        let case = format!("after deleting {}", doomed.0);
        let listed: BTreeSet<ListedPool> =
            page_through(&emulator, &case, MAX_RESULTS_MAX, expected.len())
                .await
                .into_iter()
                .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the pools still stored {expected:?}, got {listed:?}"
        );
    }
}

/// RULE: a `MaxResults` outside the modelled range never reaches the operation — the
/// constraint-validation layer ahead of it refuses the request with 400
/// `ValidationException`, naming the member, the value the request gave it and the
/// constraint that value failed — and every value inside the range, each bound included,
/// is served.
#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let emulator = Emulator::new();
    for name in ["first", "second", "third"] {
        emulator.pool(name, true).await;
    }

    for max_results in [
        MAX_RESULTS_MIN - 1,
        MAX_RESULTS_MIN - 2,
        MAX_RESULTS_MAX + 1,
        MAX_RESULTS_MAX + 40,
        i64::from(i32::MIN),
        i64::from(i32::MAX),
    ] {
        let (status, body) = emulator
            .call("ListIdentityPools", &json!({"MaxResults": max_results}))
            .await;
        assert_eq!(
            status, 400,
            "case \"MaxResults {max_results}\": expected HTTP 400 outside the modelled \
             range, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("ValidationException"),
            "case \"MaxResults {max_results}\": expected ValidationException, got \
             {code} with {body}"
        );
        let message = body["message"].as_str().unwrap_or_else(|| {
            panic!("case \"MaxResults {max_results}\": the refusal carries a message: {body}")
        });
        assert!(
            message.contains("at 'maxResults' failed to satisfy constraint"),
            "case \"MaxResults {max_results}\": expected the refusal to name the member, \
             got {message:?}"
        );
        let bound = if max_results < MAX_RESULTS_MIN {
            format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}")
        } else {
            format!("Member must have value less than or equal to {MAX_RESULTS_MAX}")
        };
        assert!(
            message.contains(&bound),
            "case \"MaxResults {max_results}\": expected the refusal to name the \
             constraint {bound:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!("Value '{max_results}'")),
            "case \"MaxResults {max_results}\": expected the refusal to echo the value \
             the request gave, got {message:?}"
        );
    }

    for max_results in [MAX_RESULTS_MIN, 2, 59, MAX_RESULTS_MAX] {
        let page = emulator
            .ok("ListIdentityPools", &json!({"MaxResults": max_results}))
            .await;
        let reported = page["IdentityPools"]
            .as_array()
            .unwrap_or_else(|| panic!("case \"MaxResults {max_results}\": a page carries an IdentityPools list: {page}"))
            .len();
        let expected = usize::try_from(max_results).unwrap_or(0).min(3);
        assert_eq!(
            reported, expected,
            "case \"MaxResults {max_results}\": expected {expected} of the 3 stored pools \
             on the page, got {reported} in {page}"
        );
    }
}

/// RULE: a `NextToken` this listing did not issue resumes nothing, so it is refused with
/// the modelled 400 `InvalidParameterException` — including a token whose pool has been
/// deleted since the token was handed out.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let pool_ids: Vec<String> = {
        let mut ids = Vec::new();
        for name in ["one", "two", "three"] {
            ids.push(emulator.pool(name, true).await);
        }
        ids
    };

    // A token this listing DID issue, and the pool it resumes at.
    let first_page = emulator
        .ok("ListIdentityPools", &json!({"MaxResults": MAX_RESULTS_MIN}))
        .await;
    let issued = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("three pools listed one at a time leave a NextToken: {first_page}"))
        .to_string();
    let resumed_at = {
        let resumed = emulator
            .ok(
                "ListIdentityPools",
                &json!({"MaxResults": MAX_RESULTS_MIN, "NextToken": &issued}),
            )
            .await;
        resumed["IdentityPools"][0]["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("the resumed page reports the pool it resumed at: {resumed}"))
            .to_string()
    };

    // A token minted for a listing INSIDE a pool does not resume the listing of pools.
    for _ in 0..2 {
        emulator.identity(&pool_ids[0], None).await;
    }
    let identities_page = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": &pool_ids[0], "MaxResults": 1}),
        )
        .await;
    let foreign = identities_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("two identities listed one at a time leave a NextToken: {identities_page}"))
        .to_string();

    let cases: Vec<(&str, String)> = vec![
        ("not hex", "not-a-token".to_string()),
        ("hex, but not a token this service minted", hex::encode("nonsense")),
        ("a token of a listing inside a pool", foreign),
        ("a token naming a pool that was never stored", hex::encode("identitypools#us-east-1:00000000-0000-4000-8000-000000000000")),
    ];
    for (case, token) in cases {
        let (status, body) = emulator
            .call(
                "ListIdentityPools",
                &json!({"MaxResults": 2, "NextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a NextToken this listing did not issue, \
             got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {code} with {body}"
        );
    }

    // The issued token still resumes the listing while its pool is stored, and stops
    // resuming anything once that pool is deleted.
    emulator
        .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &resumed_at}))
        .await;
    let (status, body) = emulator
        .call(
            "ListIdentityPools",
            &json!({"MaxResults": MAX_RESULTS_MIN, "NextToken": &issued}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"token of a deleted pool\": expected HTTP 400 once the pool the token \
         resumes at is gone, got {status} with {body}"
    );
    let code = error_code(&body);
    assert!(
        code.contains("InvalidParameterException"),
        "case \"token of a deleted pool\": expected InvalidParameterException, got {code} \
         with {body}"
    );
}
