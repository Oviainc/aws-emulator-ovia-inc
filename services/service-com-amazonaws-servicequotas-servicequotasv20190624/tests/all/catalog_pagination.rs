// SPDX-License-Identifier: CC0-1.0

//! The catalogs Service Quotas enumerates, and how a page size splits them.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, so what is exercised is the wire behaviour.
//!
//! The RULE these suites assert, over a table of listings and page sizes rather than
//! one recorded request: a Service Quotas enumeration reports the WHOLE catalog it
//! is the enumeration of — `ListServices` the integrated-service list,
//! `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` a service's whole quota list —
//! and splits it across `NextToken`-chained pages of at most `MaxResults` items, so
//! a walk with any page size the model allows (`MaxResults` @range 1..100) yields
//! that one duplicate-free listing and only the LAST page carries no `NextToken`.
//!
//! A catalog that is a handful of sample rows satisfies the wire shape and still
//! breaks the rule: every page size collapses to one page, so these suites assert
//! the catalog's EXTENT too — the service list and EC2's quota list each exceed the
//! maximum page size, the way the published catalogs do. What they do NOT require is
//! a quota row for every listed service: a service whose published defaults this
//! emulator does not carry resolves with an empty list, which
//! `default_quota_catalog.rs` pins.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::quota_workflow::{client, emulator, walk_pages_counted, Client};

/// The page sizes every walk is asserted over: the lower bound of the modelled
/// `MaxResults` @range 1..100, two sizes that divide no catalog evenly, the size the
/// recorded walks use, and the upper bound.
const PAGE_SIZES: [i64; 5] = [1, 5, 7, 10, 100];

/// Service codes the issue's checks did not cite, spanning a regional service, a
/// global one and a service whose code is not its own name — the rule is about every
/// entry of the catalog, not about `ec2`.
const UNCITED_SERVICES: [&str; 4] = ["cloudformation", "iam", "kms", "monitoring"];

/// One walked listing: every item it yielded, and how many pages it took.
struct Walk {
    items: Vec<Value>,
    pages: usize,
}

/// Walk a paginated listing to exhaustion through `quota_workflow`'s harness, which
/// asserts the page contract on the way (no page above `MaxResults`, a non-final page
/// exactly `MaxResults` long, the walk ending on a page with no `NextToken`).
async fn walk(
    client: &Client,
    operation: &str,
    base: &Value,
    items: &str,
    page_size: Option<i64>,
) -> Walk {
    let (items, pages) = walk_pages_counted(client, operation, base, items, page_size).await;
    Walk { items, pages }
}

/// The `ServiceCode`/`QuotaCode` values of a walked listing, under `member`.
fn codes(items: &[Value], member: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item[member]
                .as_str()
                .unwrap_or_else(|| panic!("case {member}: an item without one: {item}"))
                .to_string()
        })
        .collect()
}

/// Whether `value` satisfies the modelled code pattern `^[a-zA-Z][a-zA-Z0-9-]{1,<max>}$`
/// (`ServiceCode` with max 63, `QuotaCode` with max 128).
fn matches_code_pattern(value: &str, max_tail: usize) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let tail: Vec<char> = chars.collect();
    (1..=max_tail).contains(&tail.len())
        && tail
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == '-')
}

/// Every enumeration yields ONE duplicate-free listing whatever page size it is
/// walked with, and the pages are exactly as long as the page size until the last.
#[tokio::test]
async fn every_enumeration_pages_its_whole_catalog_whatever_the_page_size() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: the three enumerations, over services that differ in how many quotas
    // they publish (EC2's list exceeds the maximum page size; `cloudformation`'s is
    // shorter than the smallest page size but one).
    let mut listings: Vec<(&str, Value, &str, &str)> =
        vec![("ListServices", json!({}), "Services", "ServiceCode")];
    for service_code in ["ec2", "vpc"].iter().chain(UNCITED_SERVICES.iter()) {
        listings.push((
            "ListAWSDefaultServiceQuotas",
            json!({ "ServiceCode": service_code }),
            "Quotas",
            "QuotaCode",
        ));
        listings.push((
            "ListServiceQuotas",
            json!({ "ServiceCode": service_code }),
            "Quotas",
            "QuotaCode",
        ));
    }

    for (operation, base, items, code_member) in &listings {
        let whole = walk(&client, operation, base, items, None).await;
        assert!(
            !whole.items.is_empty(),
            "case {operation} {base}: expected a non-empty listing, got {:?}",
            whole.items
        );
        let listed = codes(&whole.items, code_member);
        let unique: BTreeSet<&String> = listed.iter().collect();
        assert_eq!(
            unique.len(),
            listed.len(),
            "case {operation} {base}: the listing repeated a {code_member}, got {listed:?}"
        );

        for size in PAGE_SIZES {
            let paged = walk(&client, operation, base, items, Some(size)).await;
            assert_eq!(
                paged.items, whole.items,
                "case {operation} {base} MaxResults={size}: paging must yield the one listing, \
                 got {} items against {}",
                paged.items.len(),
                whole.items.len()
            );
            let expected_pages = whole.items.len().div_ceil(size as usize);
            assert_eq!(
                paged.pages, expected_pages,
                "case {operation} {base} MaxResults={size}: expected {expected_pages} pages for \
                 {} items, took {}",
                whole.items.len(),
                paged.pages
            );
        }
    }
}

/// The service catalog is the integrated-service list, not a sample: it outgrows even
/// the largest page the model allows, so the documented discovery walk is a
/// multi-page one.
#[tokio::test]
async fn the_service_catalog_outgrows_the_largest_page_the_model_allows() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: the page size the recorded discovery walk uses, and the modelled maximum
    // — the catalog must take more than one page at BOTH.
    for size in [5_i64, 100] {
        let walked = walk(&client, "ListServices", &json!({}), "Services", Some(size)).await;
        let listed = codes(&walked.items, "ServiceCode");
        assert!(
            walked.pages > 1,
            "case ListServices MaxResults={size}: expected the catalog to need several pages, \
             got {} page(s) of {} services",
            walked.pages,
            listed.len()
        );
        assert!(
            listed.len() > 100,
            "case ListServices MaxResults={size}: expected the integrated-service list, got only \
             {} services",
            listed.len()
        );
    }

    let whole = walk(&client, "ListServices", &json!({}), "Services", None).await;
    let listed = codes(&whole.items, "ServiceCode");
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    assert_eq!(
        listed, sorted,
        "case ListServices: expected the catalog in ServiceCode order, got {listed:?}"
    );
    for code in &listed {
        // `ServiceCode` @length 1..63 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$.
        assert!(
            matches_code_pattern(code, 63),
            "case ListServices: {code} is not a modelled ServiceCode"
        );
    }
    for service in &whole.items {
        assert!(
            service["ServiceName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case ListServices: every service reports a ServiceName, got {service}"
        );
    }
}

/// Every service the catalog lists resolves its quota listing with well-formed,
/// duplicate-free codes, and EC2's quota list — the one the recorded walks
/// enumerate — outgrows the largest page the model allows, on both the default and
/// the applied path.
#[tokio::test]
async fn every_listed_service_resolves_and_ec2_needs_many_pages() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let services = walk(&client, "ListServices", &json!({}), "Services", None).await;
    let mut with_quotas = 0_usize;
    for service_code in codes(&services.items, "ServiceCode") {
        let base = json!({ "ServiceCode": service_code });
        // A code the catalog lists resolves: HTTP 200 with a `Quotas` list, which is
        // empty for a service whose published defaults this emulator does not carry.
        let quotas = walk(&client, "ListServiceQuotas", &base, "Quotas", Some(100)).await;
        let listed = codes(&quotas.items, "QuotaCode");
        if !listed.is_empty() {
            with_quotas += 1;
        }
        let unique: BTreeSet<&String> = listed.iter().collect();
        assert_eq!(
            unique.len(),
            listed.len(),
            "case {service_code}: two quotas share a QuotaCode, got {listed:?}"
        );
        for code in &listed {
            // `QuotaCode` @length 1..128 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$.
            assert!(
                matches_code_pattern(code, 128),
                "case {service_code}: {code} is not a modelled QuotaCode"
            );
        }
    }
    assert!(
        with_quotas > 50,
        "case catalog: expected the quota catalog to cover many services, covered {with_quotas}"
    );

    // EC2's catalog, the one the recorded checks walk: both enumerations of it take
    // several pages at MaxResults=10 AND at the modelled maximum, and the two page
    // sizes converge on the identical duplicate-free set.
    for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
        let base = json!({"ServiceCode": "ec2"});
        let mut sets: Vec<BTreeSet<String>> = Vec::new();
        for size in [10_i64, 100] {
            let walked = walk(&client, operation, &base, "Quotas", Some(size)).await;
            let listed = codes(&walked.items, "QuotaCode");
            let unique: BTreeSet<String> = listed.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                listed.len(),
                "case {operation} ec2 MaxResults={size}: the walk repeated a QuotaCode"
            );
            assert!(
                walked.pages > 1,
                "case {operation} ec2 MaxResults={size}: expected several pages, got {} for {} \
                 quotas",
                walked.pages,
                listed.len()
            );
            assert!(
                unique.len() > 100,
                "case {operation} ec2 MaxResults={size}: expected EC2's published quota list, got \
                 only {} quotas",
                unique.len()
            );
            sets.push(unique);
        }
        assert_eq!(
            sets[0], sets[1],
            "case {operation} ec2: the two page sizes must converge on one set"
        );
    }
}

/// A quota of a service the issue's checks never named still round-trips: the entry
/// the listing returned is the record the single reads return.
#[tokio::test]
async fn a_quota_the_checks_did_not_cite_round_trips_through_the_single_reads() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let mut checked = 0_usize;

    for service_code in UNCITED_SERVICES {
        let base = json!({ "ServiceCode": service_code });
        let listed = walk(&client, "ListServiceQuotas", &base, "Quotas", Some(1)).await;
        for quota in &listed.items {
            let quota_code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a code: {quota}"));
            let case = format!("{service_code}/{quota_code}");
            let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});

            let applied = client.ok("GetServiceQuota", pair.clone()).await;
            assert_eq!(
                applied["Quota"], *quota,
                "case {case}: GetServiceQuota must return the listed entry, got {applied}"
            );

            let default = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await;
            for member in ["QuotaCode", "QuotaName", "ServiceCode", "Unit", "Value"] {
                assert_eq!(
                    default["Quota"][member], quota[member],
                    "case {case}: with nothing applied the default must agree on {member}, got \
                     {default}"
                );
            }
            checked += 1;
        }
    }

    assert!(
        checked >= 4,
        "case uncited services: expected a quota from each of {UNCITED_SERVICES:?}, checked only \
         {checked}"
    );
}
