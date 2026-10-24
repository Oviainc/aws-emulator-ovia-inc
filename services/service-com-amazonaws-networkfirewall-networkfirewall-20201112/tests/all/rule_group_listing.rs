// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRuleGroups`: the listing "retrieves the metadata
//! for the rule groups that you have defined", so what it reports is whatever
//! the rule-group store holds at that moment, narrowed by the request's filters
//! and cut into the page `MaxResults`/`NextToken` ask for.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{error_code, string_at, Harness};
use crate::rule_group_lifecycle::expected_arn;

/// The `RuleGroupType` members
/// (`smithy_query(query="shape",shape="RuleGroupType")`), which is both what a
/// rule group can be created as and what `ListRuleGroups$Type` narrows on.
const RULE_GROUP_TYPES: [&str; 3] = ["STATELESS", "STATEFUL", "STATEFUL_DOMAIN"];

/// The `ResourceManagedType` members
/// (`smithy_query(query="shape",shape="ResourceManagedType")`): the categories of
/// an Amazon Web Services managed rule group, which `ListRuleGroups$ManagedType`
/// narrows on.
const MANAGED_TYPES: [&str; 4] = [
    "AWS_MANAGED_THREAT_SIGNATURES",
    "AWS_MANAGED_DOMAIN_LISTS",
    "ACTIVE_THREAT_DEFENSE",
    "PARTNER_MANAGED",
];

/// The `SubscriptionStatus` members
/// (`smithy_query(query="shape",shape="SubscriptionStatus")`).
const SUBSCRIPTION_STATUSES: [&str; 2] = ["NOT_SUBSCRIBED", "SUBSCRIBED"];

/// One page walk of `ListRuleGroups`.
#[derive(Debug)]
struct Listing {
    /// The `(Name, Arn)` pairs the pages carried, in page order.
    entries: Vec<(String, String)>,
    /// How many requests the walk took.
    pages: usize,
}

/// Walk every page of `ListRuleGroups` for `request`, asserting that a page
/// never exceeds `MaxResults`, that no entry is carried twice, and that an
/// account rule group reports no Marketplace seller.
async fn list_every_page(harness: &Harness, request: &Value, label: &str) -> Listing {
    let max_results = request
        .get("MaxResults")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(100);
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &next_token {
            page_request["NextToken"] = json!(token);
        }
        let listed = harness.ok("ListRuleGroups", &page_request).await;
        let page = listed["RuleGroups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: expected RuleGroups in {listed:?}"))
            .clone();
        assert!(
            page.len() <= max_results,
            "case {label}: a page may not exceed MaxResults={max_results}, got {page:?}"
        );
        for entry in &page {
            assert!(
                entry.get("VendorName").is_none(),
                "case {label}: a rule group this account created has no Marketplace seller, \
                 got {entry:?}"
            );
            let pair = (
                string_at(entry, &["Name"]).to_string(),
                string_at(entry, &["Arn"]).to_string(),
            );
            assert!(
                !entries.contains(&pair),
                "case {label}: {pair:?} appeared on two pages, already seen {entries:?}"
            );
            entries.push(pair);
        }
        pages += 1;
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            pages <= entries.len() + 1,
            "case {label}: pagination did not terminate, seen {entries:?}"
        );
    }
    Listing { entries, pages }
}

/// Create one rule group per `(name, type)` pair, and return the `(Name, Arn)`
/// pairs the listing must report for them, sorted.
async fn create_rule_groups(harness: &Harness, groups: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut expected: Vec<(String, String)> = Vec::new();
    for (name, group_type) in groups {
        let arn = harness.shared_rule_group(name, group_type, 10).await;
        assert_eq!(
            arn,
            expected_arn(name, group_type),
            "case ({name}, {group_type}): the create must report the rule group's own ARN"
        );
        expected.push((name.to_string(), arn));
    }
    expected.sort();
    expected
}

/// RULE: the listing reports exactly the rule groups the store holds, cut into
/// pages of at most `MaxResults` entries, with a `NextToken` present exactly
/// while entries remain and a resumed page continuing from it — no overlap and
/// no omission.
///
/// Rows span the page size against the item count, including the published
/// `PaginationMaxResults` bounds (`range 1..100`, from
/// `smithy_query(query="shape",shape="PaginationMaxResults")`) and the empty
/// store.
#[tokio::test]
async fn listing_reflects_created_rule_groups_and_pages_exactly() {
    let cases: [(usize, usize); 7] = [
        (0, 1),
        (1, 1),
        (5, 1),
        (5, 2),
        (5, 5),
        (5, 100),
        (7, 3),
    ];

    for (total, page_size) in cases {
        let harness = Harness::new();
        let names: Vec<String> = (0..total).map(|index| format!("rg-{index:02}")).collect();
        let groups: Vec<(&str, &str)> = names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), RULE_GROUP_TYPES[index % RULE_GROUP_TYPES.len()]))
            .collect();
        let expected = create_rule_groups(&harness, &groups).await;

        let label = format!("({total} rule groups, MaxResults {page_size})");
        let listing = list_every_page(
            &harness,
            &json!({ "MaxResults": page_size }),
            &label,
        )
        .await;

        let mut seen = listing.entries.clone();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {label}: expected every stored rule group exactly once, got {:?}",
            listing.entries
        );
        // Exactly the pages the page size needs: an extra page would mean a
        // `NextToken` was handed out with nothing left to resume.
        let expected_pages = total.div_ceil(page_size).max(1);
        assert_eq!(
            listing.pages, expected_pages,
            "case {label}: expected {expected_pages} page(s), took {}",
            listing.pages
        );
    }
}

/// RULE: `Type` narrows the listing to the rule groups created with that
/// `RuleGroupType`; omitting it narrows nothing.
#[tokio::test]
async fn listing_filters_by_rule_group_type() {
    let groups: [(&str, &str); 5] = [
        ("flt-stateless-a", "STATELESS"),
        ("flt-stateless-b", "STATELESS"),
        ("flt-stateful-a", "STATEFUL"),
        ("flt-domain-a", "STATEFUL_DOMAIN"),
        ("flt-domain-b", "STATEFUL_DOMAIN"),
    ];

    let harness = Harness::new();
    let expected_all = create_rule_groups(&harness, &groups).await;

    // `None` omits `Type` entirely; each row of the `RuleGroupType` catalog is
    // checked, including the one no other test cites (`STATEFUL_DOMAIN`).
    let mut cases: Vec<Option<&str>> = vec![None];
    cases.extend(RULE_GROUP_TYPES.map(Some));

    for wanted in cases {
        let mut request = json!({ "MaxResults": 100 });
        if let Some(wanted) = wanted {
            request["Type"] = json!(wanted);
        }
        let label = format!("Type={wanted:?}");
        let listing = list_every_page(&harness, &request, &label).await;
        let mut seen = listing.entries.clone();
        seen.sort();

        let expected: Vec<(String, String)> = match wanted {
            None => expected_all.clone(),
            Some(wanted) => {
                let mut kept: Vec<(String, String)> = groups
                    .iter()
                    .filter(|(_, group_type)| *group_type == wanted)
                    .map(|(name, group_type)| (name.to_string(), expected_arn(name, group_type)))
                    .collect();
                kept.sort();
                kept
            }
        };
        assert_eq!(
            seen, expected,
            "case {label}: expected only rule groups of that type, got {:?}",
            listing.entries
        );
    }
}

/// RULE: the managed-rule-group members (`Scope: MANAGED`, `ManagedType`,
/// `SubscriptionStatus`) select rule groups Amazon Web Services or a partner
/// publishes, of which this service holds none, so they list nothing; the
/// account scope — stated as `ACCOUNT` or left out — lists the account's own.
#[tokio::test]
async fn a_managed_only_filter_lists_nothing_and_the_account_scope_lists_everything() {
    let groups: [(&str, &str); 3] = [
        ("scope-stateless", "STATELESS"),
        ("scope-stateful", "STATEFUL"),
        ("scope-domain", "STATEFUL_DOMAIN"),
    ];

    let harness = Harness::new();
    create_rule_groups(&harness, &groups).await;
    let every_name = vec!["scope-domain", "scope-stateful", "scope-stateless"];

    // (the members the row sets, the names the listing must then report)
    let mut cases: Vec<(Value, Vec<&str>)> = vec![
        (json!({}), every_name.clone()),
        (json!({ "Scope": "ACCOUNT" }), every_name.clone()),
        (
            json!({ "Scope": "ACCOUNT", "Type": "STATEFUL" }),
            vec!["scope-stateful"],
        ),
        (json!({ "Scope": "MANAGED" }), vec![]),
        (json!({ "Scope": "MANAGED", "Type": "STATELESS" }), vec![]),
        (
            json!({ "Scope": "ACCOUNT", "ManagedType": "PARTNER_MANAGED" }),
            vec![],
        ),
    ];
    cases.extend(MANAGED_TYPES.map(|managed| (json!({ "ManagedType": managed }), vec![])));
    cases.extend(SUBSCRIPTION_STATUSES.map(|status| (json!({ "SubscriptionStatus": status }), vec![])));

    for (members, expected) in &cases {
        let mut request = json!({ "MaxResults": 100 });
        for (member, value) in members.as_object().expect("a row sets JSON members") {
            request[member] = value.clone();
        }
        let label = format!("{members}");
        let listing = list_every_page(&harness, &request, &label).await;
        let mut names: Vec<String> = listing
            .entries
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            names, expected,
            "case {label}: expected exactly those rule groups, got {:?}",
            listing.entries
        );
    }
}

/// RULE: the listing is a report of current state — it reflects a later create
/// and a later delete, and reading it leaves the store exactly as it was.
#[tokio::test]
async fn listing_reflects_later_creates_and_deletes_without_changing_state() {
    let harness = Harness::new();
    let expected = create_rule_groups(
        &harness,
        &[
            ("live-a", "STATELESS"),
            ("live-b", "STATEFUL"),
            ("live-c", "STATEFUL_DOMAIN"),
        ],
    )
    .await;

    let first = list_every_page(&harness, &json!({ "MaxResults": 2 }), "before").await;
    let second = list_every_page(&harness, &json!({ "MaxResults": 2 }), "again").await;
    assert_eq!(
        first.entries, second.entries,
        "case again: a listing is a read, so two consecutive walks must agree; got {:?} then {:?}",
        first.entries, second.entries
    );
    let mut sorted = first.entries.clone();
    sorted.sort();
    assert_eq!(
        sorted, expected,
        "case before: expected the three created rule groups, got {:?}",
        first.entries
    );

    // One more rule group appears in the listing...
    let added = harness.shared_rule_group("live-d", "STATEFUL", 10).await;
    let after_create = list_every_page(&harness, &json!({ "MaxResults": 100 }), "after create")
        .await
        .entries;
    assert!(
        after_create.contains(&("live-d".to_string(), added)),
        "case after create: expected the new rule group in the listing, got {after_create:?}"
    );

    // ...and a deleted one leaves it.
    harness
        .ok(
            "DeleteRuleGroup",
            &json!({ "RuleGroupName": "live-b", "Type": "STATEFUL" }),
        )
        .await;
    let after_delete: Vec<String> = list_every_page(&harness, &json!({ "MaxResults": 1 }), "after delete")
        .await
        .entries
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut names = after_delete.clone();
    names.sort();
    assert_eq!(
        names,
        vec![
            "live-a".to_string(),
            "live-c".to_string(),
            "live-d".to_string()
        ],
        "case after delete: expected the deleted rule group to be gone, got {after_delete:?}"
    );
}

/// RULE: a `NextToken` this listing did not mint is a bad request — including a
/// cursor another listing of the same service minted.
#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let harness = Harness::new();
    create_rule_groups(
        &harness,
        &[("tok-a", "STATELESS"), ("tok-b", "STATEFUL")],
    )
    .await;
    harness.create_policy("tok-policy-a").await;
    harness.create_policy("tok-policy-b").await;

    // A token this service never minted. It satisfies the modelled
    // `PaginationToken` character class, so it reaches the handler.
    let (status, body) = harness
        .call("ListRuleGroups", &json!({ "NextToken": "zzz", "MaxResults": 2 }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case foreign NextToken: expected 400 InvalidRequestException, got {status} {body:?}"
    );

    // A cursor the firewall-policy listing minted is not a rule-group cursor,
    // and the rule-group cursor is not a policy cursor.
    let policy_page = harness
        .ok("ListFirewallPolicies", &json!({ "MaxResults": 1 }))
        .await;
    let policy_token = policy_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case policy page 1: expected a NextToken in {policy_page:?}"))
        .to_string();
    let rule_group_page = harness
        .ok("ListRuleGroups", &json!({ "MaxResults": 1 }))
        .await;
    let rule_group_token = rule_group_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case rule group page 1: expected a NextToken in {rule_group_page:?}")
        })
        .to_string();

    for (target, token) in [
        ("ListRuleGroups", &policy_token),
        ("ListFirewallPolicies", &rule_group_token),
    ] {
        let (status, body) = harness
            .call(target, &json!({ "NextToken": token, "MaxResults": 2 }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case cross-listing NextToken into {target}: expected 400 \
             InvalidRequestException, got {status} {body:?}"
        );
    }
}

/// RULE: `MaxResults` is a modelled range (`1..100`); a value outside it is
/// refused at the boundary rather than clamped.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let harness = Harness::new();
    create_rule_groups(&harness, &[("bound-a", "STATELESS")]).await;

    for (max_results, inside) in [(1, true), (100, true), (0, false), (101, false)] {
        let (status, body) = harness
            .call("ListRuleGroups", &json!({ "MaxResults": max_results }))
            .await;
        if inside {
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: a published bound is accepted, got {status} \
                 {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case MaxResults={max_results}: a value past the bound is refused, got {status} \
                 {body:?}"
            );
        }
    }
}
