// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetCoipPoolUsage`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=GetCoipPoolUsage`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the usage set it answers
//! with is the account's Elastic IP addresses that were allocated OUT OF the pool `PoolId`
//! names (`Address$CustomerOwnedIpv4Pool`), narrowed by the documented `Filters` and cut
//! into `MaxResults`-sized pages, the pool and the local gateway route table it routes
//! through are read off the stored pool, and it writes nothing. A `PoolId` that resolves to
//! no pool of the account is refused rather than answered with an empty set. Every table row
//! is a distinct input along what that generalises over — which pool, how many addresses are
//! allocated out of it, which documented filter with how many values, which page size — so
//! no single recorded call is the specification.
//!
//! The pools come from the `coip_pools` suite's own fixtures, so the usage of a pool is
//! asserted against exactly the pools `DescribeCoipPools` reports.

use ec2::ops::types::Address;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_FILTERS, MAX_FILTER_VALUES,
    MAX_FILTER_VALUE_BYTES,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::coip_pools::{REGION, seed};
use super::ec2_query::{call, element, elements, error_code, error_message, filters};

/// The ec2Query action under test.
const ACTION: &str = "GetCoipPoolUsage";

/// The XML element each reported usage's allocation id arrives in
/// (`CoipAddressUsage$AllocationId`'s `xmlName`).
const ALLOCATION_ELEMENT: &str = "allocationId";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `GetCoipPoolUsage` call carrying `params`, as its HTTP status and XML body.
async fn usage(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// One `GetCoipPoolUsage` call naming `pool_id` plus `extra`, asserted to succeed, as its
/// XML body.
async fn usage_of(
    router: &MultiServiceRouter,
    case: &str,
    pool_id: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![param("PoolId", pool_id)];
    params.extend_from_slice(extra);
    let (status, body) = usage(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// The allocation ids a successful response reports, in the order it reported them.
async fn reported_allocations(
    router: &MultiServiceRouter,
    case: &str,
    pool_id: &str,
    extra: &[(String, String)],
) -> Vec<String> {
    let body = usage_of(router, case, pool_id, extra).await;
    elements(&body, ALLOCATION_ELEMENT)
}

/// One filter-table row: the case name, the `Filters` entries the request carries as
/// `(name, values)` pairs, and the allocation ids the response must report — the shape
/// `coip_pools`' own filter table uses.
type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<String>);

/// One ceiling-table row: the case name, the form fields the request carries, and the
/// allocation ids the response must report.
type CeilingCase<'a> = (&'a str, Vec<(String, String)>, Vec<String>);

/// One seeded Elastic IP address: which seeded pool it was allocated out of (`None` for an
/// address that came from Amazon's pool, so no CoIP usage may report it), the customer-owned
/// address it holds, and the service that manages it (`None` for the caller's own address).
struct AddressRow {
    pool: Option<usize>,
    customer_owned_ip: &'static str,
    service_managed: Option<&'static str>,
}

/// The addresses the usage tables are written against. They differ along everything a
/// documented filter selects on and along what a pool's usage generalises over: the first
/// pool holds seven addresses (six the caller's own, one a service's — more than the model's
/// smallest page, so a page token is minted and followed), the second holds one, the rest
/// hold none, and two addresses come from Amazon's pool instead.
fn address_rows() -> Vec<AddressRow> {
    vec![
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.4",
            service_managed: None,
        },
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.5",
            service_managed: None,
        },
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.6",
            service_managed: Some("alb"),
        },
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.7",
            service_managed: None,
        },
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.8",
            service_managed: None,
        },
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.9",
            service_managed: None,
        },
        AddressRow {
            pool: Some(0),
            customer_owned_ip: "10.0.0.10",
            service_managed: None,
        },
        AddressRow {
            pool: Some(1),
            customer_owned_ip: "10.0.1.4",
            service_managed: Some("nat-gateway"),
        },
        AddressRow {
            pool: None,
            customer_owned_ip: "",
            service_managed: None,
        },
        AddressRow {
            pool: None,
            customer_owned_ip: "",
            service_managed: None,
        },
    ]
}

/// Write the seeded addresses into the engine's store, the way `AllocateAddress` would once
/// it can draw from a customer-owned pool, and report the allocation ids they were given in
/// row order.
fn seed_addresses(engine: &engine::InMemoryEngine, pools: &[String]) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    address_rows()
        .into_iter()
        .enumerate()
        .map(|(position, row)| {
            let allocation_id = guard.next_id("eipalloc");
            let (customer_owned_ip, customer_owned_ipv4_pool) = match row.pool {
                Some(pool) => (
                    Some(row.customer_owned_ip.to_string()),
                    Some(pools[pool].clone()),
                ),
                None => (None, None),
            };
            guard.insert_address(Address {
                allocation_id: Some(allocation_id.clone()),
                public_ip: Some(format!("192.0.2.{}", position + 1)),
                domain: Some("vpc".to_string()),
                customer_owned_ip,
                customer_owned_ipv4_pool,
                service_managed: row.service_managed.map(str::to_string),
                ..Address::default()
            });
            allocation_id
        })
        .collect()
}

/// The allocation ids seeded out of the pool at `position`, in the allocation-id order the
/// operation pages its usage set in.
fn allocations_of_pool(allocations: &[String], position: usize) -> Vec<String> {
    usages_of_pool(allocations, position)
        .into_iter()
        .map(|(allocation_id, _, _)| allocation_id)
        .collect()
}

/// The usages the pool at `position` must report, as `(allocation id, customer-owned IP,
/// managing service)` in allocation-id order — the order the operation reports them in, so
/// the expectations below do not depend on the order the fixtures were written in.
fn usages_of_pool(
    allocations: &[String],
    position: usize,
) -> Vec<(String, String, Option<String>)> {
    let mut selected: Vec<(String, String, Option<String>)> = address_rows()
        .iter()
        .enumerate()
        .filter(|(_, row)| row.pool == Some(position))
        .map(|(index, row)| {
            (
                allocations[index].clone(),
                row.customer_owned_ip.to_string(),
                row.service_managed.map(str::to_string),
            )
        })
        .collect();
    selected.sort_by(|left, right| left.0.cmp(&right.0));
    selected
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A pool nothing has been allocated out of has no usage: HTTP 200 with an empty set and no
/// token, rather than a refusal. One row per seeded pool that holds no address.
#[tokio::test]
async fn a_pool_with_no_allocation_reports_no_usage() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    seed_addresses(&engine, &pools);

    // The pools at 0 and 1 are the two the fixtures allocate addresses out of, so every pool
    // after them holds none.
    for (position, pool_id) in pools.iter().enumerate().skip(2) {
        let case = format!("pool {position} ({pool_id})");
        let body = usage_of(&router, &case, pool_id, &[]).await;
        assert!(
            elements(&body, ALLOCATION_ELEMENT).is_empty(),
            "case {case:?}: expected no usage, got {body}"
        );
        assert!(
            elements(&body, "nextToken").is_empty(),
            "case {case:?}: nothing follows an empty set, so no token is due, got {body}"
        );
        assert_eq!(
            element(&body, "coipPoolId").as_deref(),
            Some(pool_id.as_str()),
            "case {case:?}: expected the pool it was asked about, got {body}"
        );
    }
}

/// Every address allocated out of the pool is reported, with the members read off that
/// address, and an address of another pool or of Amazon's pool is not. One row per seeded
/// pool, so the rows differ in how many addresses the pool holds (three, one, none).
#[tokio::test]
async fn every_address_of_the_pool_is_reported_with_its_own_members() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);

    for (position, pool_id) in pools.iter().enumerate() {
        let case = format!("pool {position} ({pool_id})");
        let body = usage_of(&router, &case, pool_id, &[]).await;
        let expected = allocations_of_pool(&allocations, position);
        assert_eq!(
            elements(&body, ALLOCATION_ELEMENT),
            expected,
            "case {case:?}: expected the pool's own allocations {expected:?}, got {body}"
        );

        // Each reported usage carries the address's own customer-owned IP and the account
        // that holds it; a service-managed address names the service, a caller's own names
        // none.
        let expected_usages = usages_of_pool(&allocations, position);
        let reported_ips = elements(&body, "coIp");
        let expected_ips: Vec<String> = expected_usages
            .iter()
            .map(|(_, co_ip, _)| co_ip.clone())
            .collect();
        assert_eq!(
            reported_ips, expected_ips,
            "case {case:?}: expected coIp {expected_ips:?}, got {reported_ips:?}"
        );
        let reported_services = elements(&body, "awsService");
        let expected_services: Vec<String> = expected_usages
            .iter()
            .filter_map(|(_, _, service)| service.clone())
            .collect();
        assert_eq!(
            reported_services, expected_services,
            "case {case:?}: expected awsService {expected_services:?}, got \
             {reported_services:?}"
        );
        for account in elements(&body, "awsAccountId") {
            assert_eq!(
                account,
                engine::emulator_account_id(),
                "case {case:?}: expected the emulator's account, got {account:?}"
            );
        }

        // "The ID of the local gateway route table" is a fact of the POOL, so it is the one
        // the describe reports for it.
        let route_table = element(&body, "localGatewayRouteTableId");
        let stored = engine
            .state::<Ec2State>()
            .lock()
            .coip_pools
            .get(pool_id)
            .and_then(|pool| pool.local_gateway_route_table_id.clone());
        assert_eq!(
            route_table, stored,
            "case {case:?}: expected the stored pool's route table {stored:?}, got \
             {route_table:?}"
        );
    }
}

/// An address allocated out of the pool AFTER a call shows up in the next one: the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn an_address_allocated_later_is_reported() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);
    let pool_id = &pools[1];

    let before = reported_allocations(&router, "before", pool_id, &[]).await;
    assert_eq!(
        before,
        allocations_of_pool(&allocations, 1),
        "before: expected the pool's one allocation, got {before:?}"
    );

    let late = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let allocation_id = guard.next_id("eipalloc");
        guard.insert_address(Address {
            allocation_id: Some(allocation_id.clone()),
            public_ip: Some("192.0.2.100".to_string()),
            domain: Some("vpc".to_string()),
            customer_owned_ip: Some("10.0.1.9".to_string()),
            customer_owned_ipv4_pool: Some(pool_id.clone()),
            ..Address::default()
        });
        allocation_id
    };

    let after = reported_allocations(&router, "after", pool_id, &[]).await;
    let mut expected = before.clone();
    expected.push(late.clone());
    expected.sort();
    assert_eq!(
        after, expected,
        "after: expected {late:?} to be reported too, got {after:?}"
    );
}

/// Reading is reporting: two identical calls report the same usages and the store is left as
/// it was.
#[tokio::test]
async fn reading_the_usage_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);

    let first = reported_allocations(&router, "first read", &pools[0], &[]).await;
    let second = reported_allocations(&router, "second read", &pools[0], &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert_eq!(
        guard.coip_pools.len(),
        pools.len(),
        "reading the usage must not write to the pool store"
    );
    assert_eq!(
        guard.addresses.len(),
        allocations.len(),
        "reading the usage must not write to the address store"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on the member it names, several values are joined with an
/// OR and several filters with an AND. The rows span every name the member documents, one
/// and two values, two filters at once, a value nothing matches, and a filter carrying no
/// value at all.
#[tokio::test]
async fn documented_filters_select_on_the_member_they_name() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);
    let account = engine::emulator_account_id().to_string();
    let all = allocations_of_pool(&allocations, 0);
    // The allocation each seeded customer-owned address belongs to, so a row names the
    // address it filters on rather than a position in a sorted list.
    let of_ip = |co_ip: &str| -> String {
        usages_of_pool(&allocations, 0)
            .into_iter()
            .find(|(_, address, _)| address == co_ip)
            .map(|(allocation_id, _, _)| allocation_id)
            .unwrap_or_else(|| panic!("no seeded allocation holds {co_ip}"))
    };
    let first_ip = of_ip("10.0.0.4");
    let second_ip = of_ip("10.0.0.5");
    let service_ip = of_ip("10.0.0.6");
    let mut two_ips = vec![first_ip.clone(), service_ip.clone()];
    two_ips.sort();

    // Each row: the case, the filter entries, and the allocation ids the response must
    // report (in allocation-id order).
    let cases: Vec<FilterCase> = vec![
        (
            "one allocation id",
            vec![("coip-address-usage.allocation-id", vec![all[0].as_str()])],
            vec![all[0].clone()],
        ),
        (
            "two allocation ids",
            vec![(
                "coip-address-usage.allocation-id",
                vec![all[0].as_str(), all[2].as_str()],
            )],
            vec![all[0].clone(), all[2].clone()],
        ),
        (
            "the account that holds them",
            vec![("coip-address-usage.aws-account-id", vec![account.as_str()])],
            all.clone(),
        ),
        (
            "a service that manages one",
            vec![("coip-address-usage.aws-service", vec!["alb"])],
            vec![service_ip.clone()],
        ),
        (
            "one customer-owned address",
            vec![("coip-address-usage.co-ip", vec!["10.0.0.5"])],
            vec![second_ip.clone()],
        ),
        (
            "two customer-owned addresses",
            vec![(
                "coip-address-usage.co-ip",
                vec!["10.0.0.4", "10.0.0.6"],
            )],
            two_ips.clone(),
        ),
        (
            "an address AND the account, both holding",
            vec![
                ("coip-address-usage.co-ip", vec!["10.0.0.4"]),
                ("coip-address-usage.aws-account-id", vec![account.as_str()]),
            ],
            vec![first_ip.clone()],
        ),
        (
            "an address AND a service that does not manage it",
            vec![
                ("coip-address-usage.co-ip", vec!["10.0.0.4"]),
                ("coip-address-usage.aws-service", vec!["alb"]),
            ],
            Vec::new(),
        ),
        (
            "an address of another pool",
            vec![("coip-address-usage.co-ip", vec!["10.0.1.4"])],
            Vec::new(),
        ),
        (
            "a filter carrying no value",
            vec![("coip-address-usage.co-ip", Vec::new())],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in cases {
        let params = filters(
            &entries
                .iter()
                .map(|(name, values)| (*name, values.as_slice()))
                .collect::<Vec<(&str, &[&str])>>(),
        );
        let reported = reported_allocations(&router, case, &pools[0], &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused, whether or not any usage would
/// have matched it — an unsupported filter must never read as "no results". Rows: a filter of
/// the sibling pool describe, a `tag:` filter this member documents none of, the member name
/// without its `coip-address-usage.` prefix, and the empty name.
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    seed_addresses(&engine, &pools);

    for name in [
        "coip-pool.pool-id",
        "tag:Name",
        "allocation-id",
        "coip-address-usage.co_ip",
        "",
    ] {
        let params = filters(&[(name, &["anything"])]);
        let mut all = vec![param("PoolId", &pools[0])];
        all.extend(params);
        let (status, body) = usage(&router, &all).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// `MaxResults` cuts the usage set into pages and `NextToken` walks them: every page carries
/// at most the asked-for count, following the tokens reports every usage exactly once in
/// order, and the last page carries no token. Rows: the model's minimum page size, one
/// between, and a size at least as large as the whole set — the first two smaller than the
/// seeded set, so a token is actually minted and followed.
#[tokio::test]
async fn max_results_pages_the_usage_set_and_next_token_walks_it() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);
    let expected = allocations_of_pool(&allocations, 0);

    // `CoipPoolMaxResults` is `range 5..1000`, so 5 is the smallest page the model accepts.
    // The pool holds more than that, so 5 and 6 must both take more than one page while 1000
    // reports the whole set in one.
    for (page_size, expected_pages) in [(5, 2), (6, 2), (1000, 1)] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case {case:?}: paging did not end after {pages} pages"
            );
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(param("NextToken", cursor));
            }
            let body = usage_of(&router, &case, &pools[0], &params).await;
            let page = elements(&body, ALLOCATION_ELEMENT);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} usages in a page, got {page:?}"
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) over {} usages, got {pages}",
            expected.len()
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every usage exactly once in order, got {walked:?}"
        );
    }
}

/// A `NextToken` this service did not mint is refused rather than read as a position in the
/// set. Rows: free text, an allocation id sent raw, and the empty token.
#[tokio::test]
async fn a_bogus_next_token_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);

    for (case, token) in [
        ("free text", "not-a-token".to_string()),
        ("a raw allocation id", allocations[0].clone()),
        ("the empty token", String::new()),
    ] {
        let (status, body) = usage(
            &router,
            &[param("PoolId", &pools[0]), param("NextToken", &token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// `MaxResults` outside the range the model publishes is refused at the boundary, and each
/// published bound itself is answered. The rows are each published bound and one past it.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    seed_addresses(&engine, &pools);

    // The bounds `GetCoipPoolUsageRequest$MaxResults` publishes (`CoipPoolMaxResults`,
    // `range 5..1000`), and one past each:
    // smithy_query(query="shape",shape="CoipPoolMaxResults",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
    for (max_results, expected_status) in [(4, 400), (5, 200), (1000, 200), (1001, 400)] {
        let (status, body) = usage(
            &router,
            &[
                param("PoolId", &pools[0]),
                param("MaxResults", &max_results.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ceilings on one request's filtering work
// ---------------------------------------------------------------------------

/// Matching cost is the PRODUCT of how many filters a request carries, how many values each
/// carries, how long a value is and how many addresses the store holds, and the model bounds
/// none of them (`FilterList` and `Filter$Values` carry no `@length`). So a request AT each
/// of this crate's ceilings is answered, and one PAST any of them is refused with
/// `InvalidParameterValue` and reports no usage. Each row is a distinct factor of that
/// product, named by the constant it is derived from rather than by a literal.
#[tokio::test]
async fn a_request_at_each_filtering_ceiling_is_answered_and_past_it_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    let allocations = seed_addresses(&engine, &pools);
    let every_usage = allocations_of_pool(&allocations, 0);

    // Every seeded customer-owned address of the first pool is in `10.0.0.0/24`, so the
    // wildcard and the value list below both select all of them: a ceiling case asserts the
    // WHOLE set comes back, not merely that the request was answered.
    let at_filter_ceiling: Vec<(&str, &[&str])> = (0..MAX_FILTERS)
        .map(|_| ("coip-address-usage.co-ip", &["10.0.0.*"][..]))
        .collect();
    let many_values: Vec<String> = (0..MAX_FILTER_VALUES).map(|n| format!("10.0.0.{n}")).collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);

    let accepted: Vec<CeilingCase> = vec![
        (
            "MAX_FILTERS filters",
            filters(&at_filter_ceiling),
            every_usage.clone(),
        ),
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("coip-address-usage.co-ip", &many_values)]),
            every_usage.clone(),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("coip-address-usage.co-ip", &[long_value.as_str()])]),
            Vec::new(),
        ),
    ];
    for (case, params, expected) in accepted {
        let reported = reported_allocations(&router, case, &pools[0], &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} at the ceiling, got {reported:?}"
        );
    }

    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("coip-address-usage.co-ip", &["10.0.0.*"][..]))
        .collect();
    let past_value_ceiling: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|n| format!("10.0.0.{n}"))
        .collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let past_byte_ceiling = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);

    let refused: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "one filter past MAX_FILTERS",
            filters(&past_filter_ceiling),
        ),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("coip-address-usage.co-ip", &past_value_ceiling)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("coip-address-usage.co-ip", &[past_byte_ceiling.as_str()])]),
        ),
    ];
    for (case, params) in refused {
        let mut all = vec![param("PoolId", &pools[0])];
        all.extend(params);
        let (status, body) = usage(&router, &all).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 past the ceiling, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            elements(&body, ALLOCATION_ELEMENT).is_empty(),
            "case {case:?}: a refused request must report no usage, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// A PoolId that resolves to no pool
// ---------------------------------------------------------------------------

/// A `PoolId` that resolves to no pool of the account is refused, and the CODE says HOW the
/// value failed: a well-formed pool id nothing was created for is `InvalidPoolID.NotFound`,
/// a value that READS as a pool id but carries a digit count EC2 never mints is
/// `InvalidParameterValue`, and a value that is no pool id at all is
/// `InvalidPoolID.Malformed` — the one reading of a `CoipPoolId` this whole surface shares.
/// The rows are each way a value can fail to resolve.
#[tokio::test]
async fn a_pool_id_that_resolves_to_no_pool_is_refused() {
    let prefix = ec2::state::COIP_POOL_ID_PREFIX;
    let not_found = ec2::state::INVALID_POOL_ID_NOT_FOUND;
    let bad_value = ec2::state::INVALID_PARAMETER_VALUE;
    let malformed = ec2::state::INVALID_POOL_ID_MALFORMED;
    let cases = [
        ("the long form", format!("{prefix}00000000000000000"), not_found),
        ("the short form", format!("{prefix}00000000"), not_found),
        ("one digit", format!("{prefix}0"), bad_value),
        ("neither length", format!("{prefix}0000000000000000"), bad_value),
        ("an upper-case suffix", format!("{prefix}0ABCDEF01"), malformed),
        ("a non-hex suffix", format!("{prefix}zzzzzzzz"), malformed),
        (
            "another resource's id",
            "subnet-00000000000000000".to_string(),
            malformed,
        ),
        ("free text", "not-a-pool-at-all".to_string(), malformed),
    ];

    for (case, pool_id, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let pools = seed(&engine);
        seed_addresses(&engine, &pools);

        let (status, body) = usage(&router, &[param("PoolId", &pool_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        if expected_code != malformed {
            assert_eq!(
                error_message(&body),
                format!("The pool ID '{pool_id}' does not exist."),
                "case {case:?}: expected the surface's one sentence, got {body}"
            );
        }
    }
}

/// A request that carries no `PoolId` at all names no pool to report on, so it is refused
/// rather than answered with an empty usage set. The member is `@required`, so the boundary
/// validator is what refuses it — before this operation's handler runs.
#[tokio::test]
async fn a_request_that_names_no_pool_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    seed_addresses(&engine, &pools);

    let (status, body) = usage(&router, &[]).await;
    assert_eq!(
        status, 400,
        "no PoolId: expected HTTP 400, got {status} with body {body}"
    );
    assert!(
        elements(&body, ALLOCATION_ELEMENT).is_empty(),
        "no PoolId: a refused request must report no usage, got {body}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and reads nothing,
/// whether or not the pool it names exists; `DryRun=false` is an ordinary read.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome() {
    let (router, engine) = super::ec2_query::emulator();
    let pools = seed(&engine);
    seed_addresses(&engine, &pools);
    let unknown = format!("{}00000000000000000", ec2::state::COIP_POOL_ID_PREFIX);

    // Rows: a pool the store holds, and an id it does not — the flag asks whether the caller
    // is permitted, which is not a question about the pool.
    for (case, pool_id) in [
        ("a stored pool", pools[0].as_str()),
        ("an unknown pool", unknown.as_str()),
    ] {
        let (status, body) = usage(
            &router,
            &[param("PoolId", pool_id), param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }

    // `DryRun=false` is no dry run at all: the usage is reported.
    let (status, body) = usage(
        &router,
        &[param("PoolId", &pools[0]), param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        !elements(&body, ALLOCATION_ELEMENT).is_empty(),
        "DryRun=false: expected the pool's usage to be reported, got {body}"
    );
}
