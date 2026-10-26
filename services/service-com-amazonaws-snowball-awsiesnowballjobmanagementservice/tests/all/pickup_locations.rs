// SPDX-License-Identifier: CC0-1.0

//! `ListPickupLocations` (and the `CreateAddress` that persists what it lists), driven end
//! to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `ListPickupLocations` reports one `Address` per address this account holds that is typed
//! as a pickup address, in the order the addresses were created, paging them with
//! `MaxResults` as the page size and a `NextToken` that resumes the walk where the previous
//! page stopped; every reported address is the record `DescribeAddress` reports under that
//! id, and an address typed as a delivery address is not reported at all. A token naming no
//! pickup location this store holds is refused with the error this operation models.
//!
//! The rows differ along what the rule generalises over: the `AddressType` (every value of
//! the modelled catalog, plus an address that declares none, so the type the create
//! defaults to is covered), which optional members the address carries (every documented
//! member, only the members a carrier needs, the two members the model documents as
//! ignored), the length and script of the values, whether the address is restricted, how
//! many pickup locations the store holds (none, one, several — the listing is walked again
//! after every create), the page size (absent, each published bound of the modelled range,
//! sizes that divide the listing and sizes that do not) and the form of the token
//! (unminted, truncated, altered, naming a delivery address, naming no address at all).
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`) is the one `job_lifecycle` and `long_term_pricing_listing` already drive the
//! service with, so every call here goes through the same front door a client uses and
//! nothing reaches into the state struct.

use serde_json::{json, Value};
// The type a create stores when the caller declares none, the catalog the modelled
// `AddressType` admits, the value that names a pickup address and the one page-size rule
// every list operation of this service pages by, read from the crate rather than restated,
// so a row cannot drift from the behaviour it exercises.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_address::DEFAULT_ADDRESS_TYPE;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::ADDRESS_TYPE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    encode_cursor, page_size, ADDRESS_TYPE_CUSTOMER_PICKUP,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{create_address, deliverable_address, ok, refused, router, string};

/// `ListPickupLocationsRequest$MaxResults` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListPickupLocations")`), so these are the
/// smallest and largest page sizes the member admits.
const MAX_RESULTS_MIN: i32 = 0;
const MAX_RESULTS_MAX: i32 = 100;

/// The `Address` members the model documents as "This field is no longer used and the value
/// is ignored" (`smithy_query(query="shape", shape="Address")`), so a submitted value is
/// neither stored nor reported.
const IGNORED_ADDRESS_MEMBERS: &[&str] = &["PrefectureOrDistrict", "Landmark"];

/// A well-formed `AddressId` this store holds no address under — the form the modelled
/// pattern `^ADID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` admits.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// Whether the rule under test expects the address `submitted` to be reported as a pickup
/// location: the stored `Address$Type` is the submitted one, or the type a create defaults
/// to when the caller declared none, and a pickup location is an address typed as one.
fn is_pickup_location(submitted: &Value) -> bool {
    submitted
        .get("Type")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_ADDRESS_TYPE)
        == ADDRESS_TYPE_CUSTOMER_PICKUP
}

/// Every address row: one declaring no `Type`, then, for each value of the modelled
/// catalog, one carrying every documented member, one carrying only the members a carrier
/// needs, and one restricted address with long and non-ASCII values.
fn rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![(
        "no Type, so the type a create stores by default".to_string(),
        deliverable_address("Untyped Receiver", "1 Default Way", json!({})),
    )];
    for address_type in ADDRESS_TYPE_VALUES {
        rows.push((
            format!("{address_type} carrying every documented member"),
            deliverable_address(
                "Full Receiver",
                "2 Catalog Way",
                json!({
                    "Company": "Example Corp",
                    "Street2": "Suite 400",
                    "Street3": "Mailstop 7",
                    "StateOrProvince": "WA",
                    "IsRestricted": false,
                    "Type": address_type,
                    "PrefectureOrDistrict": "an ignored prefecture",
                    "Landmark": "an ignored landmark",
                }),
            ),
        ));
        rows.push((
            format!("{address_type} carrying only the members a carrier needs"),
            deliverable_address(
                "Minimal Receiver",
                "3 Short Road",
                json!({ "Type": address_type }),
            ),
        ));
        rows.push((
            format!("{address_type}, restricted, with long and non-ascii values"),
            deliverable_address(
                "Núñez Ólafsdóttir",
                &"x".repeat(1024),
                json!({
                    "Company": "ドコモ・システムズ株式会社",
                    "City": "São Paulo del Norte",
                    "StateOrProvince": "WA",
                    "IsRestricted": true,
                    "Type": address_type,
                }),
            ),
        ));
    }
    rows
}

/// The `Addresses` of one response, as a list.
fn addresses(listed: &Value) -> Vec<Value> {
    listed
        .get("Addresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One `ListPickupLocations` call with `max_results` and `next_token` on it, returning the
/// addresses of the page and the `NextToken` it carried, if any.
async fn one_page(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let mut body = json!({});
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    let listed = ok(router, "ListPickupLocations", body, case).await;
    let token = listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (addresses(&listed), token)
}

/// Every pickup location the listing reports, walked page by page with `max_results` as the
/// page size until it hands back no token.
async fn walk(router: &MultiServiceRouter, max_results: Option<i32>, case: &str) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let (page, token) = one_page(router, max_results, next_token.as_deref(), case).await;
        assert!(
            page.len() <= page_size(max_results),
            "case {case:?}: expected at most state::page_size({max_results:?}) addresses on \
             a page, got {}",
            page.len()
        );
        listed.extend(page);
        next_token = token;
        if next_token.is_none() {
            break listed;
        }
        assert!(
            listed.len() <= usize::try_from(MAX_RESULTS_MAX).expect("a positive bound"),
            "case {case:?}: this suite seeds fewer locations than the published maximum \
             page, so a walk that has reported {} is not terminating",
            listed.len()
        );
    }
}

/// The `AddressId` of each reported location, in the order reported.
fn reported_ids(listed: &[Value], case: &str) -> Vec<String> {
    listed
        .iter()
        .map(|address| string(address, "AddressId", case))
        .collect()
}

/// Assert the listing reports exactly `expected` — the ids of the pickup locations the store
/// holds, in creation order — and that every reported address is the record
/// `DescribeAddress` reports under its id, typed as a pickup address and carrying neither
/// member the model documents as ignored.
async fn assert_reports_the_pickup_locations(
    router: &MultiServiceRouter,
    expected: &[String],
    case: &str,
) {
    let listed = walk(router, Some(1), case).await;
    assert_eq!(
        reported_ids(&listed, case),
        expected.to_vec(),
        "case {case:?}: expected the pickup locations the store holds in creation order, got \
         {listed:?}"
    );

    for address in &listed {
        let address_id = string(address, "AddressId", case);
        assert_eq!(
            address.get("Type").and_then(Value::as_str),
            Some(ADDRESS_TYPE_CUSTOMER_PICKUP),
            "case {case:?}: expected every reported location to be typed as a pickup \
             address, got {address:?}"
        );
        for member in IGNORED_ADDRESS_MEMBERS {
            assert!(
                address.get(*member).is_none(),
                "case {case:?}: expected no {member} on a reported location, got {address:?}"
            );
        }
        let described = ok(
            router,
            "DescribeAddress",
            json!({ "AddressId": address_id }),
            case,
        )
        .await;
        assert_eq!(
            described.get("Address"),
            Some(address),
            "case {case:?}: expected the reported location to be the stored address \
             DescribeAddress reports, got {described}"
        );
    }
}

#[tokio::test]
async fn list_pickup_locations_reports_the_pickup_addresses_the_store_holds() {
    let router = router();

    // A store in which no address has been created holds no pickup location, and reports
    // an empty listing rather than a token or an invented catalog.
    let (empty, token) = one_page(&router, None, None, "a store holding no address").await;
    assert!(
        empty.is_empty() && token.is_none(),
        "expected an empty listing with no token from a store holding no address, got \
         {empty:?} and {token:?}"
    );

    // The listing is walked again after every create, so each row is asserted against the
    // store as it stood at that point: one pickup location, then several.
    let mut expected: Vec<String> = Vec::new();
    for (case, submitted) in rows() {
        let address_id = create_address(&router, submitted.clone(), &case).await;
        if is_pickup_location(&submitted) {
            expected.push(address_id);
        }
        assert_reports_the_pickup_locations(&router, &expected, &case).await;
    }
    assert!(
        expected.len() > 1,
        "this suite needs more than one pickup location to tell a page from a listing, got \
         {}",
        expected.len()
    );

    // A read changes nothing a later caller sees: the same walk reports the same locations.
    assert_reports_the_pickup_locations(&router, &expected, "the listing walked again").await;
}

#[tokio::test]
async fn list_pickup_locations_pages_by_the_shared_page_size_rule() {
    let router = router();
    let mut expected: Vec<String> = Vec::new();
    for (case, submitted) in rows() {
        let address_id = create_address(&router, submitted.clone(), &case).await;
        if is_pickup_location(&submitted) {
            expected.push(address_id);
        }
    }
    let total = expected.len();

    // Absent, each published bound of the modelled range, and sizes that do and do not
    // divide the listing.
    let mut sizes: Vec<Option<i32>> = vec![None, Some(MAX_RESULTS_MIN)];
    for size in 1..=i32::try_from(total + 1).expect("a small listing") {
        sizes.push(Some(size));
    }
    sizes.push(Some(MAX_RESULTS_MAX));

    for max_results in sizes {
        let case = format!("MaxResults {max_results:?}");
        let on_the_page = page_size(max_results).min(total);
        let (page, token) = one_page(&router, max_results, None, &case).await;
        assert_eq!(
            page.len(),
            on_the_page,
            "case {case:?}: expected state::page_size({max_results:?}) locations bounded by \
             the {total} the listing holds, got {}",
            page.len()
        );
        assert_eq!(
            token.is_some(),
            on_the_page < total,
            "case {case:?}: expected a NextToken exactly when locations remain after a page \
             of {on_the_page} of {total}, got {token:?}"
        );

        // Whatever the page size, the whole walk reports every pickup location once, in
        // creation order.
        let listed = walk(&router, max_results, &case).await;
        assert_eq!(
            reported_ids(&listed, &case),
            expected,
            "case {case:?}: expected the whole walk to report every pickup location once in \
             creation order, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn list_pickup_locations_refuses_a_token_it_did_not_mint_for_this_listing() {
    let router = router();
    let mut pickup: Vec<String> = Vec::new();
    let mut delivery: Vec<String> = Vec::new();
    for (case, submitted) in rows() {
        let address_id = create_address(&router, submitted.clone(), &case).await;
        if is_pickup_location(&submitted) {
            pickup.push(address_id);
        } else {
            delivery.push(address_id);
        }
    }
    assert!(
        pickup.len() > 1 && !delivery.is_empty(),
        "this suite needs several pickup locations and a delivery address, got {pickup:?} \
         and {delivery:?}"
    );

    let (first, token) = one_page(&router, Some(1), None, "the first page of one").await;
    assert_eq!(
        first.len(),
        1,
        "expected a page of one location to carry one, got {first:?}"
    );
    let token = token.expect("a listing with locations after its first page carries a token");

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
            "a cursor naming a delivery address rather than a pickup location".to_string(),
            encode_cursor(&delivery[0]),
        ),
        (
            "a truncated cursor".to_string(),
            token[..token.len() / 2].to_string(),
        ),
        ("an altered cursor".to_string(), altered),
    ];
    for (case, next_token) in rows {
        refused(
            &router,
            "ListPickupLocations",
            json!({ "NextToken": next_token }),
            "InvalidResourceException",
            &case,
        )
        .await;
    }

    // The token this listing did mint still resumes the walk where the first page stopped.
    let case = "resuming with the minted token";
    let (resumed, _) = one_page(&router, Some(1), Some(&token), case).await;
    assert_eq!(
        reported_ids(&resumed, case),
        vec![pickup[1].clone()],
        "expected the resumed page to start at the location the first page stopped before, \
         got {resumed:?}"
    );
}
