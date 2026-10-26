// SPDX-License-Identifier: CC0-1.0

//! `GetCustomerGatewayAssociations` pinned as a PAGED reporting read, driven over HTTP.
//!
//! The rule under test: the reader answers the stored ties of the global network its URI
//! addresses as a sequence of pages whose size is the caller's `MaxResults` — a page carries
//! at most that many ties, carries a resume token exactly while ties remain, and following
//! those tokens visits every stored tie of that global network exactly once in one stable
//! order — a page size outside the modelled range of that member is refused with the
//! operation's modelled `ValidationException` and no page at all, the resource the path
//! addresses is resolved before any selector or token the query string carries, and none of
//! these reads changes what a later caller sees.
//!
//! `customer_gateway_association_lifecycle` tables the slice's round trip, its ARN selector
//! and the one page size of one; this suite crosses the page size itself — the whole
//! modelled range, its published bounds and the values past them — with the selector and
//! with the page boundary, which that table does not. Every call goes through the real
//! `MultiServiceRouter`, and nothing reaches into `crate::state`: the ties are created and
//! read back only through operations.

use serde_json::json;

use crate::customer_gateway_association_lifecycle::{
    arns_of, associate, device_with_link, get_associations, plain_cgw_arn,
};
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::escape;

/// How many ties the fixture stores: enough that a page size inside the modelled range
/// splits them several ways, and well under the per-global-network bound
/// (`MAX_CUSTOMER_GATEWAY_ASSOCIATIONS_PER_GLOBAL_NETWORK`).
const TIE_COUNT: usize = 12;

/// The published bounds of the page size: `MaxResults` is `range 1..500`
/// (`smithy_query query="operation" operation="GetCustomerGatewayAssociations"`).
const SMALLEST_PAGE_SIZE: i64 = 1;
const LARGEST_PAGE_SIZE: i64 = 500;

/// The ARN of the fixture tie `index`, built so the lexicographic order of the ARNs — the
/// order the store keys the ties in — is the order of `index`.
fn fixture_arn(index: usize) -> String {
    plain_cgw_arn(&format!("{index:08x}"))
}

/// Store `TIE_COUNT` ties on one device of a fresh global network, and answer the global
/// network's id together with the ARNs of the ties, in the order the store keys them.
async fn fixture(emulator: &Emulator, label: &str) -> (String, Vec<String>) {
    let network = emulator.create_global_network(label).await;
    let subject = device_with_link(emulator, &network, "hq").await;
    let mut stored = Vec::new();
    for index in 0..TIE_COUNT {
        let arn = fixture_arn(index);
        let (status, created) = associate(
            emulator,
            &network,
            json!({ "CustomerGatewayArn": arn, "DeviceId": subject.device }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case fixture tie {index}: expected 200 from AssociateCustomerGateway, got {status} with {created}"
        );
        stored.push(arn);
    }
    stored.sort();
    (network, stored)
}

/// The query string a read of `network` carries: the ARN `filter` (empty for none), the page
/// `size` (absent for none) and the resume `token` (absent on the first page).
fn query(filter: &[String], size: Option<i64>, token: Option<&str>) -> String {
    let mut parts: Vec<String> = filter
        .iter()
        .map(|arn| format!("customerGatewayArns={}", escape(arn)))
        .collect();
    if let Some(size) = size {
        parts.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        parts.push(format!("nextToken={}", escape(token)));
    }
    parts.join("&")
}

/// Follow every page of the read of `network` under `filter` and page `size`, starting at
/// `from` (the first page when it is `None`), answering the ARNs visited in the order they
/// were answered.
///
/// Asserts the per-page half of the rule as it walks: a page never exceeds the size asked
/// for, a page carrying a token is never empty, and only the final page carries none.
async fn walk_pages(
    case: &str,
    emulator: &Emulator,
    network: &str,
    filter: &[String],
    size: Option<i64>,
    from: Option<&str>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = from.map(str::to_string);
    // One page per tie is the most any size inside the range can need; the bound is what
    // turns a token that fails to advance into a failure instead of a hang.
    for page_number in 0..=TIE_COUNT {
        let (status, page) =
            get_associations(emulator, network, &query(filter, size, token.as_deref())).await;
        assert_eq!(
            status, 200,
            "case {case} page {page_number}: expected 200, got {status} with {page}"
        );
        let arns = arns_of(&page);
        if let Some(size) = size {
            assert!(
                i64::try_from(arns.len()).is_ok_and(|taken| taken <= size),
                "case {case} page {page_number}: a page must carry at most {size} ties, got {page}"
            );
        }
        token = page["NextToken"].as_str().map(str::to_string);
        assert!(
            token.is_none() || !arns.is_empty(),
            "case {case} page {page_number}: a page that resumes must not be empty, got {page}"
        );
        seen.extend(arns);
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case}: the tokens never ran out after {TIE_COUNT} pages, having seen {seen:?}")
}

/// Every page size inside the modelled range partitions the global network's ties: each page
/// holds at most that many, and the tokens visit all of them exactly once, in key order.
#[tokio::test]
async fn every_page_size_in_the_modelled_range_partitions_the_stored_ties() {
    let emulator = Emulator::new();
    let (network, stored) = fixture(&emulator, "page sizes").await;

    // The rows span what the rule generalises over — the page size: absent (one page with
    // everything), the published minimum, sizes that leave a remainder and sizes that divide
    // the set exactly, one below / exactly / one above the number of ties stored (the page
    // boundary, where a token must stop being issued), and the published maximum.
    let sizes: [(&str, Option<i64>); 9] = [
        ("no page size at all", None),
        ("the smallest page size the model allows", Some(SMALLEST_PAGE_SIZE)),
        ("a page size that divides the ties exactly", Some(4)),
        ("a page size that leaves a remainder", Some(5)),
        ("one page short of the whole set", Some(TIE_COUNT as i64 - 1)),
        ("exactly the whole set", Some(TIE_COUNT as i64)),
        ("one past the whole set", Some(TIE_COUNT as i64 + 1)),
        ("a page size far past the whole set", Some(123)),
        ("the largest page size the model allows", Some(LARGEST_PAGE_SIZE)),
    ];
    for (case, size) in sizes {
        let seen = walk_pages(case, &emulator, &network, &[], size, None).await;
        assert_eq!(
            seen, stored,
            "case {case}: following every token must visit every stored tie exactly once in key order, got {seen:?}"
        );
    }

    // The selector and the page size compose: a filtered read is paged over the ties the
    // filter selects and nothing else, whatever the page size.
    let selected: Vec<String> = stored.iter().step_by(3).cloned().collect();
    for size in [SMALLEST_PAGE_SIZE, 2, 3, LARGEST_PAGE_SIZE] {
        let case = format!("a filtered read with a page size of {size}");
        let seen = walk_pages(&case, &emulator, &network, &selected, Some(size), None).await;
        assert_eq!(
            seen, selected,
            "case {case}: a paged filtered read must visit exactly the selected ties, got {seen:?}"
        );
    }

    // None of the reads above changed what a later caller sees.
    let (status, page) = get_associations(&emulator, &network, "maxResults=500").await;
    assert_eq!(
        status, 200,
        "case read after every walk: expected 200, got {status} with {page}"
    );
    assert_eq!(
        arns_of(&page),
        stored,
        "case read after every walk: a read must not change what a later caller sees, got {page}"
    );
}

/// A page size outside the modelled range is refused, naming the member at fault and
/// carrying no page — and the refusal leaves the stored ties exactly where they were.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_and_carries_no_page() {
    let emulator = Emulator::new();
    let (network, stored) = fixture(&emulator, "page size bounds").await;

    // The rows are the values on each side of the published bounds: one below the minimum,
    // one above the maximum, and values far outside on both sides.
    let rows: [(&str, i64); 4] = [
        ("one below the smallest page size", SMALLEST_PAGE_SIZE - 1),
        ("a negative page size", -1),
        ("one above the largest page size", LARGEST_PAGE_SIZE + 1),
        ("a page size far above the largest", 100_000),
    ];
    for (case, size) in rows {
        let (status, refused) =
            get_associations(&emulator, &network, &query(&[], Some(size), None)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused}"
        );
        // The constraint is the modelled one, so the refusal is the framework's: its
        // message travels under the wire name of the member, not the modelled one.
        let message = refused["Message"]
            .as_str()
            .or_else(|| refused["message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("maxResults"),
            "case {case}: the refusal must name the member at fault, got {refused}"
        );
        assert!(
            refused["CustomerGatewayAssociations"].is_null(),
            "case {case}: a refused request must carry no page, got {refused}"
        );
    }

    let (status, page) = get_associations(&emulator, &network, "maxResults=500").await;
    assert_eq!(
        status, 200,
        "case read after the refusals: expected 200, got {status} with {page}"
    );
    assert_eq!(
        arns_of(&page),
        stored,
        "case read after the refusals: a refused read must leave the ties untouched, got {page}"
    );
}

/// The global network in the path is resolved BEFORE any selector or token the query string
/// carries, so a read of one that is not stored is the missing-resource answer whatever else
/// the request asks for.
#[tokio::test]
async fn an_unknown_global_network_is_not_found_whatever_the_query_string_carries() {
    let emulator = Emulator::new();
    let (network, stored) = fixture(&emulator, "path first").await;
    // An id of the minted shape (`<kind>-<17 lowercase hex>`) that names nothing: the
    // leading `f` puts it above anything the monotonic id counter can mint.
    let unknown_network = "global-network-f0000000000000001";

    // The resume token of a REAL page of a real global network, so the row that carries it
    // asks with a token this service itself issued.
    let (status, first) = get_associations(&emulator, &network, "maxResults=1").await;
    assert_eq!(
        status, 200,
        "case fixture first page: expected 200, got {status} with {first}"
    );
    let issued = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case fixture first page: expected a NextToken in {first}"))
        .to_string();

    // The rows span the query strings that would otherwise decide the answer: nothing, a
    // selector, a selector past its documented maximum of 10 values, a page size, a token
    // this service issued and a forged one. A page size outside the modelled range is not a
    // row: that bound is modelled, so the request never reaches the operation at all.
    //
    // The missing-resource answer wins over each of them because the global network is what
    // the URI ADDRESSES (the operation's `@http` uri is
    // `/global-networks/{GlobalNetworkId}/customer-gateway-associations`): every selector,
    // bound and token in the query string narrows or resumes a listing OF that global
    // network, so there is nothing for them to be judged against until it resolves.
    let over_long: Vec<String> = (0..11).map(fixture_arn).collect();
    let rows: Vec<(&str, String)> = vec![
        ("no query string", String::new()),
        ("an ARN selector", query(&stored[..1], None, None)),
        ("a selector past its maximum", query(&over_long, None, None)),
        ("a page size", query(&[], Some(SMALLEST_PAGE_SIZE), None)),
        ("a token this service issued", query(&[], None, Some(&issued))),
        ("a forged token", "nextToken=not-a-token".to_string()),
    ];
    for (case, uri_query) in &rows {
        let (status, refused) = get_associations(&emulator, unknown_network, uri_query).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            refused["ResourceId"],
            json!(unknown_network),
            "case {case}: the refusal must name the global network asked for, got {refused}"
        );
        assert!(
            refused["CustomerGatewayAssociations"].is_null(),
            "case {case}: a refused request must carry no page, got {refused}"
        );
    }

    // The same token read against the global network that issued it still resumes, so the
    // refusals above are about the path and nothing else.
    let (status, resumed) =
        get_associations(&emulator, &network, &query(&[], None, Some(&issued))).await;
    assert_eq!(
        status, 200,
        "case the issuing global network: expected 200, got {status} with {resumed}"
    );
    assert_eq!(
        arns_of(&resumed),
        stored[1..],
        "case the issuing global network: the token must resume after the first tie, got {resumed}"
    );
}

/// A resume token selects the position after the last tie of the page that issued it, and
/// nothing else, so a client may change the page size between pages and still see every
/// remaining tie exactly once.
#[tokio::test]
async fn a_token_resumes_after_its_page_under_a_different_page_size() {
    let emulator = Emulator::new();
    let (network, stored) = fixture(&emulator, "resume").await;

    // The rows are the (first size, second size) pairs: a small page followed by a large
    // one, a large one followed by a small one, and the same size twice.
    for (first_size, second_size) in [(2_i64, LARGEST_PAGE_SIZE), (5, 1), (3, 3)] {
        let case = format!("a page of {first_size} resumed with {second_size}");
        let (status, first) =
            get_associations(&emulator, &network, &query(&[], Some(first_size), None)).await;
        assert_eq!(
            status, 200,
            "case {case} first page: expected 200, got {status} with {first}"
        );
        let taken = usize::try_from(first_size).expect("the page size must be non-negative");
        assert_eq!(
            arns_of(&first),
            stored[..taken],
            "case {case} first page: expected the first {first_size} ties, got {first}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case} first page: expected a NextToken in {first}"))
            .to_string();

        // Walking on from that token at the SECOND size reaches every remaining tie, once
        // each, in the same order — the token fixes a position, not a page size.
        let rest = walk_pages(
            &case,
            &emulator,
            &network,
            &[],
            Some(second_size),
            Some(&token),
        )
        .await;
        assert_eq!(
            rest,
            stored[taken..],
            "case {case}: resuming at the second size must reach every remaining tie, got {rest:?}"
        );

        let (status, resumed) = get_associations(
            &emulator,
            &network,
            &query(&[], Some(second_size), Some(&token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case} resumed: expected 200, got {status} with {resumed}"
        );
        let expected_len = (stored.len() - taken).min(
            usize::try_from(second_size).expect("the page size must be non-negative"),
        );
        assert_eq!(
            arns_of(&resumed),
            stored[taken..taken + expected_len],
            "case {case} resumed: the token must resume right after its own page, got {resumed}"
        );
    }
}
