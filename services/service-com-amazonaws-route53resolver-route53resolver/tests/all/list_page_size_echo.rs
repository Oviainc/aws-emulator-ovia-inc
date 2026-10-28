// SPDX-License-Identifier: CC0-1.0

//! A `@paginated` list reports the page size it applied.
//!
//! The rule under test: every list operation whose response models `MaxResults`
//! reports there the page size the answer was cut to — the value the request
//! asked for, or the documented default when the request named none ("If you
//! don't specify a value for `MaxResults`, Resolver returns up to 100 …", the
//! input member's own doc on each of these operations) — so a paginating client
//! always reads an integer there, and never an unset optional.
//!
//! The operations are exactly the four whose `ListResolver…Response` carries a
//! `MaxResults` member (`smithy_query(query="operation",operation=
//! "ListResolverEndpoints,ListResolverEndpointIpAddresses,ListResolverRules,\
//! ListResolverRuleAssociations")`), each with the modelled range 1..100; the
//! list operations whose response models no `MaxResults` report none, which this
//! suite also pins.
//!
//! The rows differ along what the rule generalises over: the operation, the
//! requested page size (both published bounds, a size between them, one that
//! divides the collection and one that does not, and `MaxResults` omitted), and
//! whether the page is the first, a resumed one or the last.
//!
//! Every case goes over the wire the service actually speaks, through the shared
//! [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::DEFAULT_PAGE_SIZE;

use super::harness::{emulator, ok, string_member, Emulator};

/// The bounds the model puts on `MaxResults` for these four operations
/// (`range 1..100`), which are also the sizes a request may ask for.
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 100);

/// The page size a request that names no `MaxResults` gets, as an `i64`.
fn default_page_size() -> i64 {
    i64::try_from(DEFAULT_PAGE_SIZE).expect("the default page size fits in an i64")
}

/// Create one endpoint with `count` addresses and return its `Id`.
async fn seed_endpoint(router: &Emulator, creator: &str, count: usize) -> String {
    let subnets: Vec<String> = (0..count)
        .map(|index| format!("subnet-{creator}-{index}"))
        .collect();
    router.hold_subnets(&subnets);
    let ip_addresses: Vec<Value> = subnets
        .iter()
        .enumerate()
        .map(|(index, subnet)| json!({ "SubnetId": subnet, "Ip": format!("10.0.{index}.10") }))
        .collect();
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": "OUTBOUND",
            "SecurityGroupIds": ["sg-00000000000000001"],
            "IpAddresses": ip_addresses,
        }),
    )
    .await;
    string_member(
        created
            .get("ResolverEndpoint")
            .expect("the create answers a ResolverEndpoint"),
        "Id",
    )
}

/// Create one FORWARD rule through `endpoint` and associate it with `vpc`.
///
/// The VPC is registered in the account's own Amazon EC2 store first: a rule is
/// associated WITH a VPC of the account, which the associate resolves against
/// that store.
async fn seed_rule(router: &Emulator, creator: &str, endpoint: &str, vpc: &str) {
    router.hold_vpcs([vpc]);
    let created = ok(
        router,
        "CreateResolverRule",
        json!({
            "CreatorRequestId": creator,
            "Name": creator,
            "RuleType": "FORWARD",
            "DomainName": format!("{creator}.example.com"),
            "ResolverEndpointId": endpoint,
            "TargetIps": [{ "Ip": "192.0.2.10" }],
        }),
    )
    .await;
    let rule_id = string_member(
        created
            .get("ResolverRule")
            .expect("the create answers a ResolverRule"),
        "Id",
    );
    ok(
        router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": vpc, "Name": creator }),
    )
    .await;
}

/// One row of the table: a list operation, the member its page of items is under
/// and the request members that select the collection.
struct ListRow {
    operation: &'static str,
    items: &'static str,
    request: Value,
}

/// Every one of the four operations reports the page size it applied, on every
/// page, for a requested size and for none at all.
#[tokio::test]
async fn a_list_reports_the_page_size_it_applied() {
    let router = emulator();
    // A collection of five of each kind, so a page size of two neither divides
    // it nor covers it in one page.
    let endpoint = seed_endpoint(&router, "page-size", 5).await;
    for index in 0..4 {
        seed_endpoint(&router, &format!("page-size-more-{index}"), 2).await;
    }
    for index in 0..5 {
        seed_rule(
            &router,
            &format!("rule-page-size-{index}"),
            &endpoint,
            &format!("vpc-0000000000000000{index}"),
        )
        .await;
    }

    let rows = [
        ListRow {
            operation: "ListResolverEndpoints",
            items: "ResolverEndpoints",
            request: json!({}),
        },
        ListRow {
            operation: "ListResolverEndpointIpAddresses",
            items: "IpAddresses",
            request: json!({ "ResolverEndpointId": endpoint }),
        },
        ListRow {
            operation: "ListResolverRules",
            items: "ResolverRules",
            request: json!({}),
        },
        ListRow {
            operation: "ListResolverRuleAssociations",
            items: "ResolverRuleAssociations",
            request: json!({}),
        },
    ];

    for row in &rows {
        // Page sizes spanning the modelled range: both published bounds, a size
        // that divides the collection, one that does not, and none at all.
        for max_results in [
            Some(MAX_RESULTS_BOUNDS.0),
            Some(2),
            Some(5),
            Some(MAX_RESULTS_BOUNDS.1),
            None,
        ] {
            let expected = max_results.unwrap_or_else(default_page_size);
            let mut next_token: Option<String> = None;
            let mut page_number = 0;
            loop {
                let mut request = row.request.clone();
                if let Some(size) = max_results {
                    request["MaxResults"] = json!(size);
                }
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let case = format!(
                    "{} MaxResults={max_results:?} page {page_number}",
                    row.operation
                );
                let page = ok(&router, row.operation, request).await;
                page_number += 1;
                assert!(
                    page_number <= 10,
                    "case {case:?}: paging did not terminate, got {page}"
                );

                assert_eq!(
                    page.get("MaxResults").and_then(Value::as_i64),
                    Some(expected),
                    "case {case:?}: MaxResults must report the page size applied, got {page}"
                );
                let items = page
                    .get(row.items)
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                assert!(
                    i64::try_from(items).unwrap_or(i64::MAX) <= expected,
                    "case {case:?}: a page must hold at most the page size applied, got {page}"
                );

                next_token = page
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if next_token.is_none() {
                    break;
                }
            }
            assert!(
                page_number > 1 || expected >= 5,
                "case {}: a page size below the collection must take more than one page",
                row.operation
            );
        }
    }
}

/// A list operation whose response models no `MaxResults` reports none: the rule
/// is about the member the model declares, not about every list.
#[tokio::test]
async fn a_list_whose_response_models_no_page_size_reports_none() {
    let router = emulator();
    ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-no-echo", "Name": "no-echo" }),
    )
    .await;

    for operation in [
        "ListFirewallDomainLists",
        "ListResolverQueryLogConfigs",
        "ListFirewallRuleGroups",
    ] {
        let listed = ok(&router, operation, json!({ "MaxResults": 7 })).await;
        assert!(
            listed.get("MaxResults").is_none(),
            "case {operation:?}: the response models no MaxResults, so none may be \
             reported, got {listed}"
        );
    }
}
