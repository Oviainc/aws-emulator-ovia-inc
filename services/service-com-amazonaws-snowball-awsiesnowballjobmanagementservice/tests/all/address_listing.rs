// SPDX-License-Identifier: CC0-1.0

//! `DescribeAddresses`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `DescribeAddresses` reports one `Address` per address this account holds — every address
//! `CreateAddress` stored, in the order they were created, whichever Region each call is
//! signed for — paging them with `MaxResults` as the page size and a `NextToken` that
//! resumes the walk where the previous page stopped, and each entry reports the address
//! `DescribeAddress` reports under the same id; a token this service did not mint for this
//! listing is refused with `InvalidNextTokenException`.
//!
//! The rows differ along what the rule generalises over: how many addresses the store holds
//! (none, one, several — the listing is walked again after every create), which members the
//! address carries (only the ones a carrier needs, every documented member), the value each
//! member of the modelled `AddressType` catalog takes, whether the address is restricted,
//! the length of a stored value at the published bound of its member, the page size
//! (absent, each published bound of the modelled range, sizes that divide the listing and
//! sizes that do not), the Region a call is signed for, and the form of the token
//! (unminted, truncated, altered, another listing's).
//!
//! A row that needs a well-formed token naming a record of its choosing mints it with the
//! crate's own cursor codec (`state::encode_cursor`), so the suite cannot drift from the
//! encoding the handler decodes.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `complete_order`, `string`, `assert_address_matches`) is the one
//! `job_lifecycle` and `address_validation` already drive the service with, so every call
//! here goes through the same front door a client uses and nothing reaches into the state
//! struct.

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::ADDRESS_TYPE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::encode_cursor;
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    assert_address_matches, call_in_region, complete_order, create_address, deliverable_address,
    ok, refused, router, string,
};

/// `DescribeAddressesRequest$MaxResults` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="DescribeAddresses")`), so these are the
/// smallest and largest page sizes the member admits.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// `Address$Company` is modelled `length 1..1024`
/// (`smithy_query(query="shape", shape="Address", member="Company")`), so this is the
/// longest value the member admits.
const ADDRESS_MEMBER_MAX_LEN: usize = 1024;

/// The Region most calls of this suite sign for, in the SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// Another Region of the same shipping area: the listing is the account's, so a call signed
/// for it reports exactly the same addresses.
const OTHER_REGION: &str = "us-east-2";

/// An address id in the modelled `AddressId` form —
/// `^ADID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="shape", shape="AddressId")`) — that names no stored address.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// Every listing row: the addresses the store is grown with, one per way an address can
/// differ — the members it carries, each value of the modelled type catalog, whether it is
/// restricted, and a value at the published maximum length of its member.
fn rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "only the members a carrier needs".to_string(),
            deliverable_address("Minimal Lister", "1 Listing Way", json!({})),
        ),
        (
            "every documented member".to_string(),
            deliverable_address(
                "Complete Lister",
                "2 Listing Way",
                json!({
                    "Company": "Example Corp",
                    "Street2": "Suite 400",
                    "Street3": "Mailstop 7",
                    "StateOrProvince": "WA",
                    "IsRestricted": false,
                }),
            ),
        ),
        (
            "a restricted address".to_string(),
            deliverable_address(
                "Restricted Lister",
                "3 Listing Way",
                json!({ "IsRestricted": true }),
            ),
        ),
        (
            "a value at the published maximum length".to_string(),
            deliverable_address(
                "Long Lister",
                "4 Listing Way",
                json!({ "Company": "C".repeat(ADDRESS_MEMBER_MAX_LEN) }),
            ),
        ),
    ];
    // One row per value the modelled `AddressType` enum admits, so the listing is asserted
    // over addresses of each type rather than over one type.
    for address_type in ADDRESS_TYPE_VALUES {
        rows.push((
            format!("an address typed {address_type}"),
            deliverable_address(
                "Typed Lister",
                "5 Listing Way",
                json!({ "Type": address_type }),
            ),
        ));
    }
    rows
}

/// The page sizes the listing is walked with: absent, each published bound of the modelled
/// range, and sizes that do and do not divide the listing.
fn page_sizes() -> Vec<(String, Option<i64>)> {
    let mut sizes: Vec<(String, Option<i64>)> = vec![
        ("no MaxResults".to_string(), None),
        (
            "MaxResults at the published minimum".to_string(),
            Some(MAX_RESULTS_MIN),
        ),
        (
            "MaxResults at the published maximum".to_string(),
            Some(MAX_RESULTS_MAX),
        ),
    ];
    for size in [1i64, 2, 3, 5] {
        sizes.push((format!("MaxResults {size}"), Some(size)));
    }
    sizes
}

/// The addresses one `DescribeAddresses` answer reports, reading an answer that reports
/// none as an empty listing: a store holding no address has no entry to report.
fn entries(listed: &Value) -> Vec<Value> {
    listed
        .get("Addresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Walk `DescribeAddresses` in `region` with `max_results` as the page size, returning the
/// addresses in the order the pages reported them.
async fn page_through(
    router: &MultiServiceRouter,
    region: &str,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let (status, listed) = call_in_region(router, region, "DescribeAddresses", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAddresses to answer 200, got {status} with \
             {listed}"
        );
        let page = entries(&listed);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: expected at most {} pages for {expected_total} addresses, got \
             {pages}",
            expected_total + 1
        );
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = &next_token {
            // `NextToken` is modelled `len 1..1024`, so a token this operation mints is one
            // the next request can carry.
            assert!(
                (1..=1024).contains(&token.chars().count()),
                "case {case:?}: expected a NextToken of 1..1024 characters, got {token:?}"
            );
        }
        // A declared page size bounds the page, and a page with addresses after it is
        // full: anything less would make the walk report fewer addresses per page than
        // asked for while still having more to report.
        if let Some(max) = max_results.filter(|max| *max > 0) {
            let max = usize::try_from(max).expect("a positive page size");
            assert!(
                page.len() <= max,
                "case {case:?}: expected at most {max} entries in a page, got {}",
                page.len()
            );
            if next_token.is_some() {
                assert_eq!(
                    page.len(),
                    max,
                    "case {case:?}: expected a page with addresses after it to be full, got {}",
                    page.len()
                );
            }
        }
        seen.extend(page);
        if next_token.is_none() {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn describe_addresses_pages_over_every_stored_address_in_creation_order() {
    let router = router();

    // A store holding no address has nothing to report and no page after this one.
    let empty_case = "a store holding no address";
    let empty = ok(&router, "DescribeAddresses", json!({}), empty_case).await;
    assert!(
        entries(&empty).is_empty(),
        "case {empty_case:?}: expected no entry, got {empty}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case {empty_case:?}: expected no NextToken on the last page, got {empty}"
    );

    // The listing is walked again after every create, so the rule is asserted over a store
    // holding one address, two, and so on up to the whole table.
    let mut created: Vec<(String, Value)> = Vec::new();
    for (row, submitted) in rows() {
        let address_id = create_address(&router, submitted.clone(), &row).await;
        created.push((address_id, submitted));

        for (size_case, max_results) in page_sizes() {
            for region in [SIGNED_REGION, OTHER_REGION] {
                let case =
                    format!("{} stored addresses, {size_case}, signed for {region} ({row})", created.len());
                let seen =
                    page_through(&router, region, max_results, created.len(), &case).await;
                let seen_ids: Vec<String> = seen
                    .iter()
                    .map(|address| string(address, "AddressId", &case))
                    .collect();
                let expected_ids: Vec<String> = created
                    .iter()
                    .map(|(address_id, _)| address_id.clone())
                    .collect();
                assert_eq!(
                    seen_ids, expected_ids,
                    "case {case:?}: expected the pages to report every stored address once, \
                     in creation order, got {seen_ids:?}"
                );
                // Each reported address is the one that was created under that id, with
                // nothing invented and nothing dropped.
                for (reported, (address_id, submitted)) in seen.iter().zip(created.iter()) {
                    assert_address_matches(reported, submitted, address_id, &case);
                }
            }
        }
    }

    // Each entry reports the address `DescribeAddress` reports under the same id: the
    // listing and the single read are built from one record.
    let one_page = ok(
        &router,
        "DescribeAddresses",
        json!({ "MaxResults": MAX_RESULTS_MAX }),
        "the whole listing on one page",
    )
    .await;
    let listed = entries(&one_page);
    assert_eq!(
        listed.len(),
        created.len(),
        "expected every created address to be listed once, got {one_page}"
    );
    for entry in &listed {
        let address_id = string(entry, "AddressId", "the whole listing on one page");
        let case = format!("the entry for {address_id}");
        let described = ok(
            &router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            &case,
        )
        .await;
        assert_eq!(
            entry,
            &described["Address"],
            "case {case:?}: DescribeAddresses and DescribeAddress disagree: {entry} vs \
             {described}"
        );
    }

    // A page size outside the modelled range is refused before the handler runs.
    for (case, max_results) in [
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
    ] {
        let (status, parsed) = call_in_region(
            &router,
            SIGNED_REGION,
            "DescribeAddresses",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected MaxResults {max_results} to be refused, got {status} \
             with {parsed}"
        );
    }
}

#[tokio::test]
async fn describe_addresses_refuses_a_token_it_did_not_mint_for_this_listing() {
    let router = router();

    // Three addresses, so this listing has pages after its first, and one job, so the store
    // also holds a record a cursor of ANOTHER listing can name.
    let mut addresses: Vec<String> = Vec::new();
    for index in 0..3usize {
        let case = format!("address {index} of the token suite");
        addresses.push(
            create_address(
                &router,
                deliverable_address(
                    "Token Lister",
                    &format!("{} Token Way", index + 1),
                    json!({}),
                ),
                &case,
            )
            .await,
        );
    }
    let job_case = "the job of the token suite";
    let created_job = ok(
        &router,
        "CreateJob",
        complete_order(&json!({ "AddressId": addresses[0] })),
        job_case,
    )
    .await;
    let job_id = string(&created_job, "JobId", job_case);

    let first_case = "the first page of the address listing";
    let first_page = ok(
        &router,
        "DescribeAddresses",
        json!({ "MaxResults": 1 }),
        first_case,
    )
    .await;
    let token = string(&first_page, "NextToken", first_case);

    let mut altered = token.clone();
    altered.pop();
    altered.push(if token.ends_with('0') { '1' } else { '0' });

    let rows: Vec<(String, String)> = vec![
        ("not a cursor".to_string(), "garbage".to_string()),
        (
            "a cursor for an address that does not exist".to_string(),
            encode_cursor(UNKNOWN_ADDRESS_ID),
        ),
        (
            "a truncated cursor".to_string(),
            token[..token.len() / 2].to_string(),
        ),
        ("an altered cursor".to_string(), altered),
        (
            "a cursor naming a job rather than an address".to_string(),
            encode_cursor(&job_id),
        ),
    ];
    for (case, next_token) in rows {
        refused(
            &router,
            "DescribeAddresses",
            json!({ "NextToken": next_token }),
            "InvalidNextTokenException",
            &case,
        )
        .await;
    }

    // The token this listing did mint still resumes the walk where the first page stopped.
    let second_page = ok(
        &router,
        "DescribeAddresses",
        json!({ "MaxResults": 1, "NextToken": token }),
        "resuming with the minted token",
    )
    .await;
    let resumed = entries(&second_page);
    assert_eq!(
        resumed.len(),
        1,
        "expected the minted token to resume the walk, got {second_page}"
    );
    assert_eq!(
        string(&resumed[0], "AddressId", "the resumed page"),
        addresses[1],
        "expected the resumed page to start at the address the first page stopped before, \
         got {second_page}"
    );
}

#[tokio::test]
async fn walking_the_listing_leaves_the_store_as_it_found_it() {
    let router = router();

    // The rule is a reporting one: however many times the listing is walked, and with
    // whatever page size, the addresses it reports do not change and no address is
    // consumed, renamed or re-ordered by having been read.
    let mut created: Vec<String> = Vec::new();
    for (index, (row, submitted)) in rows().into_iter().enumerate() {
        let case = format!("{row} (repeat-read row {index})");
        created.push(create_address(&router, submitted, &case).await);
    }

    let baseline = page_through(
        &router,
        SIGNED_REGION,
        Some(MAX_RESULTS_MAX),
        created.len(),
        "the first walk",
    )
    .await;
    assert_eq!(
        baseline.len(),
        created.len(),
        "expected the first walk to report every stored address, got {baseline:?}"
    );

    for (size_case, max_results) in page_sizes() {
        let case = format!("re-walking with {size_case}");
        let again = page_through(
            &router,
            SIGNED_REGION,
            max_results,
            created.len(),
            &case,
        )
        .await;
        assert_eq!(
            again, baseline,
            "case {case:?}: expected the listing to report the same addresses in the same \
             order, got {again:?}"
        );
    }

    // And every address is still readable one at a time, so nothing was spent by listing.
    for address_id in &created {
        let case = format!("reading {address_id} back after the walks");
        let described = ok(
            &router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            &case,
        )
        .await;
        assert_eq!(
            described["Address"]["AddressId"].as_str(),
            Some(address_id.as_str()),
            "case {case:?}: expected the address to still be readable, got {described}"
        );
    }
}
