// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFirewallRuleGroups`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].
//!
//! What these cases pin is the paging contract of a `@paginated` list operation
//! over the collection this one reports: the rule groups this account has
//! defined, in creation order, walked exactly once whatever the page size, with
//! a cursor only while items remain, and each entry narrowed to the members
//! `FirewallRuleGroupMetadata` declares.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member, ACCOUNT, REGION};

/// The members `FirewallRuleGroupMetadata` declares, from the model
/// (`smithy_query(query="shape",shape="FirewallRuleGroupMetadata")`): the
/// "minimal high-level information" a listing reports.
const METADATA_MEMBERS: &[&str] = &[
    "Id",
    "Arn",
    "Name",
    "OwnerId",
    "CreatorRequestId",
    "ShareStatus",
];

/// The members `FirewallRuleGroup` carries and `FirewallRuleGroupMetadata` does
/// not, which is why the metadata doc says to "call GetFirewallRuleGroup and
/// ListFirewallRules" for the full information.
const FULL_ONLY_MEMBERS: &[&str] = &[
    "RuleCount",
    "Status",
    "StatusMessage",
    "CreationTime",
    "ModificationTime",
];

/// The `FirewallRuleGroups` of one page.
fn page_items(body: &Value) -> Vec<Value> {
    body.get("FirewallRuleGroups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the response carries a FirewallRuleGroups array, got {body}"))
}

/// Walk every page of `ListFirewallRuleGroups` for `request`, returning the
/// items in the order they were reported.
async fn walk(router: &MultiServiceRouter, request: &Value, case: &str) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut body = request.clone();
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, "ListFirewallRuleGroups", body).await;
        pages += 1;
        assert!(
            pages <= 200,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        let items = page_items(&page);
        if let Some(size) = request.get("MaxResults").and_then(Value::as_i64) {
            assert!(
                i64::try_from(items.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: a page must hold at most MaxResults items, got {page}"
            );
        }
        walked.extend(items);

        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    walked
}

/// Create `count` rule groups and return the created `FirewallRuleGroup`s in
/// creation order.
async fn seed_groups(router: &MultiServiceRouter, count: usize, prefix: &str) -> Vec<Value> {
    let mut made = Vec::new();
    for index in 0..count {
        let created = ok(
            router,
            "CreateFirewallRuleGroup",
            json!({
                "CreatorRequestId": format!("req-{prefix}-{index}"),
                "Name": format!("{prefix}-{index}"),
            }),
        )
        .await;
        made.push(
            created
                .get("FirewallRuleGroup")
                .expect("the create response carries a FirewallRuleGroup")
                .clone(),
        );
    }
    made
}

/// An account that has defined no rule group is reported an empty collection —
/// the list member present and no cursor — rather than a refusal or a group the
/// emulator invented. DNS Firewall publishes no rule groups of its own, so there
/// is nothing to report before the first create.
#[tokio::test]
async fn an_account_with_no_rule_groups_is_reported_an_empty_list() {
    let router = emulator();

    // The rows span the ways a caller can ask for the first page: no paging
    // members at all, and each published bound of `MaxResults`.
    for (case, request) in [
        ("MaxResults omitted", json!({})),
        ("the published minimum", json!({ "MaxResults": 1 })),
        ("the published maximum", json!({ "MaxResults": 100 })),
    ] {
        let page = ok(&router, "ListFirewallRuleGroups", request).await;
        assert_eq!(
            page_items(&page),
            Vec::<Value>::new(),
            "case {case:?}: an account that defined no rule group must be reported an empty list, got {page}"
        );
        assert_eq!(
            page.get("NextToken"),
            None,
            "case {case:?}: a page that ends the collection carries no cursor, got {page}"
        );
    }

    // Creating a domain list does not make a rule group: the two collections are
    // separate, so the listing still reports nothing.
    ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-list-only", "Name": "domains-only" }),
    )
    .await;
    let page = ok(&router, "ListFirewallRuleGroups", json!({})).await;
    assert_eq!(
        page_items(&page),
        Vec::<Value>::new(),
        "case \"only a domain list exists\": a domain list is not a rule group, got {page}"
    );
}

/// One row per created group: every rule group this account creates is reported
/// by the listing, with the members the metadata shape declares taken from the
/// record the create wrote, and the full-group members left to
/// `GetFirewallRuleGroup`.
///
/// The table spans what the rule generalises over — the shape of `Name` (letters,
/// digits beside letters, underscore, hyphen, space, apostrophe, the one- and
/// 64-character bounds) and `Tags` present, empty and absent, none of which the
/// listing reports differently.
#[tokio::test]
async fn every_created_group_is_reported_with_the_members_of_its_record() {
    let longest_name = "g".repeat(64);
    let cases = vec![
        (
            "letters only, no Tags",
            json!({ "CreatorRequestId": "req-letters", "Name": "blocklist-group" }),
        ),
        (
            "digits beside letters and an underscore",
            json!({
                "CreatorRequestId": "req-mixed",
                "Name": "group_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
        ),
        (
            "spaces and an apostrophe",
            json!({ "CreatorRequestId": "req-spaces", "Name": "ops' rule group" }),
        ),
        (
            "one character",
            json!({ "CreatorRequestId": "req-shortest", "Name": "g", "Tags": [] }),
        ),
        (
            "the longest Name the model allows",
            json!({ "CreatorRequestId": "req-longest", "Name": longest_name }),
        ),
    ];

    let router = emulator();
    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, body) in cases {
        let response = ok(&router, "CreateFirewallRuleGroup", body).await;
        created.push((
            case,
            response
                .get("FirewallRuleGroup")
                .expect("the create response carries a FirewallRuleGroup")
                .clone(),
        ));
    }

    let listed = walk(&router, &json!({}), "every created group").await;
    assert_eq!(
        listed.len(),
        created.len(),
        "case \"every created group\": the listing must report each created group once, got {listed:?}"
    );

    for ((case, made), item) in created.iter().zip(listed.iter()) {
        for member in METADATA_MEMBERS {
            assert_eq!(
                item.get(member),
                made.get(member),
                "case {case:?}: {member} must be the created record's, got {item}"
            );
        }
        for member in FULL_ONLY_MEMBERS {
            assert_eq!(
                item.get(member),
                None,
                "case {case:?}: {member} is not a member of FirewallRuleGroupMetadata, got {item}"
            );
        }

        let id = string_member(item, "Id");
        assert_eq!(
            string_member(item, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-rule-group/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and this id, got {item}"
        );
        assert_eq!(
            string_member(item, "OwnerId"),
            ACCOUNT,
            "case {case:?}: OwnerId is the account that created the group, got {item}"
        );
        assert_eq!(
            string_member(item, "ShareStatus"),
            "NOT_SHARED",
            "case {case:?}: a group this account created is shared with nobody, got {item}"
        );

        // The id the listing reported resolves to the same group through the
        // reader, so one id names one resource across the service.
        let fetched = ok(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
        )
        .await;
        let read = fetched
            .get("FirewallRuleGroup")
            .expect("the read carries a FirewallRuleGroup");
        for member in METADATA_MEMBERS {
            assert_eq!(
                read.get(member),
                item.get(member),
                "case {case:?}: the read must agree with the listing on {member}, got {fetched}"
            );
        }
    }
}

/// Listing is a pure read: walking twice reports the very same entries, and a
/// create in between only appends past what a caller already read.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    seed_groups(&router, 3, "stable").await;

    let first = walk(&router, &json!({}), "a first walk").await;
    let second = walk(&router, &json!({}), "a second walk").await;
    assert_eq!(
        first, second,
        "case \"a second walk\": listing twice must report the same entries"
    );

    // Paging through one item at a time is a read as well, so the collection is
    // unchanged after it.
    let paged = walk(&router, &json!({ "MaxResults": 1 }), "one at a time").await;
    assert_eq!(
        paged, first,
        "case \"one at a time\": paging must report the same entries as one page"
    );

    let added = seed_groups(&router, 1, "appended").await;
    let third = walk(&router, &json!({}), "after a create").await;
    assert_eq!(
        third[..first.len()],
        first[..],
        "case \"after a create\": the entries already reported must stay where they were, got {third:?}"
    );
    assert_eq!(
        third.len(),
        first.len() + 1,
        "case \"after a create\": the create must add exactly one entry, got {third:?}"
    );
    assert_eq!(
        string_member(&third[first.len()], "Id"),
        string_member(&added[0], "Id"),
        "case \"after a create\": the created group must be the entry added, got {third:?}"
    );
}

/// A rule group the account deleted is no longer reported: the listing is cut
/// out of the records the store holds now, not an append-only log of creates.
#[tokio::test]
async fn a_deleted_rule_group_is_no_longer_reported() {
    let router = emulator();
    let seeded = seed_groups(&router, 3, "deleted").await;
    let ids: Vec<String> = seeded
        .iter()
        .map(|group| string_member(group, "Id"))
        .collect();

    // One row per position the deleted group holds in the collection — the
    // middle one first, then the head, then the only one left — so the rule is
    // checked wherever the page is cut, not only at the tail.
    for (case, index) in [("the middle entry", 1), ("the first entry", 0)] {
        let removed = ids[index].clone();
        ok(
            &router,
            "DeleteFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": removed.clone() }),
        )
        .await;

        let listed: Vec<String> = walk(&router, &json!({}), case)
            .await
            .iter()
            .map(|item| string_member(item, "Id"))
            .collect();
        assert!(
            !listed.contains(&removed),
            "case {case:?}: the deleted group {removed} must not be reported, got {listed:?}"
        );
        // The groups beside it are untouched, and paging one at a time reports
        // the same collection as one page.
        let paged: Vec<String> = walk(&router, &json!({ "MaxResults": 1 }), case)
            .await
            .iter()
            .map(|item| string_member(item, "Id"))
            .collect();
        assert_eq!(
            paged, listed,
            "case {case:?}: paging must report the collection the one-page walk did"
        );
    }

    let last = ids[2].clone();
    ok(
        &router,
        "DeleteFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": last }),
    )
    .await;
    let page = ok(&router, "ListFirewallRuleGroups", json!({})).await;
    assert_eq!(
        page_items(&page),
        Vec::<Value>::new(),
        "case \"every group deleted\": an account left with none is reported an empty list, got {page}"
    );
}

/// Paging walks every rule group exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_rule_group_exactly_once() {
    let router = emulator();
    let seeded = seed_groups(&router, 7, "paged").await;
    let expected: Vec<String> = seeded
        .iter()
        .map(|group| string_member(group, "Id"))
        .collect();

    // Page sizes spanning the modelled MaxResults range (1..100): both published
    // bounds, sizes that do and do not divide the collection, exactly the
    // collection size, and MaxResults omitted altogether.
    for max_results in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut request = json!({});
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        let walked: Vec<String> = walk(&router, &request, &case)
            .await
            .iter()
            .map(|item| string_member(item, "Id"))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every rule group exactly once, in creation order"
        );
    }

    // A page that ends the collection carries no cursor, and one that does not
    // carries one — so a caller knows when to stop without a second empty call.
    let first = ok(
        &router,
        "ListFirewallRuleGroups",
        json!({ "MaxResults": 1 }),
    )
    .await;
    assert!(
        first.get("NextToken").is_some(),
        "case \"a page with items left over\": a cursor is owed, got {first}"
    );
    let whole = ok(
        &router,
        "ListFirewallRuleGroups",
        json!({ "MaxResults": 100 }),
    )
    .await;
    assert_eq!(
        whole.get("NextToken"),
        None,
        "case \"a page that ends the collection\": no cursor is owed, got {whole}"
    );
}

/// `MaxResults` is modelled `range 1..100`, so a request outside it is refused
/// rather than clamped into a page the caller did not ask for.
///
/// The refusal is the boundary validator's, so only its HTTP status is asserted:
/// a handler-returned error and a boundary one differ in the error code they
/// render (see `crate::scaffolded`).
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    seed_groups(&router, 2, "bounded").await;

    for (case, max_results, accepted) in [
        ("one below the published minimum", 0, false),
        ("far below the published minimum", -1, false),
        ("the published minimum", 1, true),
        ("the published maximum", 100, true),
        ("one above the published maximum", 101, false),
        ("far above the published maximum", 10_000, false),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallRuleGroups",
            json!({ "MaxResults": max_results }),
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: MaxResults={max_results} must answer HTTP {expected}, got {status}: {body}"
        );
    }
}

/// A `NextToken` this service did not mint for this collection is refused with
/// the `ValidationException` the operation models, rather than silently read as
/// "start over" and answering page one again.
#[tokio::test]
async fn a_next_token_this_query_did_not_mint_is_refused() {
    let router = emulator();
    seed_groups(&router, 4, "cursor").await;

    let page = ok(
        &router,
        "ListFirewallRuleGroups",
        json!({ "MaxResults": 1 }),
    )
    .await;
    let minted = page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a page with items left over carries a cursor")
        .to_string();

    // A cursor another list operation minted is not an offset into this
    // collection: it is seeded here over the wire, so it really is a token this
    // service issued — for another query.
    ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-other-collection-0", "Name": "other-0" }),
    )
    .await;
    ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-other-collection-1", "Name": "other-1" }),
    )
    .await;
    let other = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 1 }),
    )
    .await
    .get("NextToken")
    .and_then(Value::as_str)
    .expect("the domain-list listing has items left over")
    .to_string();

    // Rows spanning the ways a cursor can fail to be this query's: nothing like
    // a cursor, a cursor whose payload was edited, a truncated one, a plausible
    // plaintext offset nobody minted, and a cursor minted for another collection.
    let tampered = {
        let mut bytes = minted.clone().into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        String::from_utf8(bytes).expect("still ASCII")
    };
    for (case, token) in [
        ("not a cursor at all", "not-a-cursor".to_string()),
        ("an empty string", String::new()),
        ("a tampered cursor", tampered),
        (
            "the minted cursor with its tail cut off",
            minted[..minted.len() - 4].to_string(),
        ),
        (
            "a plaintext offset",
            hex::encode("ListFirewallRuleGroups/all/v1:2"),
        ),
        ("a cursor minted for another collection", other),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallRuleGroups",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this query did not mint must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );
    }

    // The cursor this query did mint still resumes the walk, so the refusals
    // above are about the token and not about refusing to resume.
    let resumed = ok(
        &router,
        "ListFirewallRuleGroups",
        json!({ "MaxResults": 1, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        page_items(&resumed).len(),
        1,
        "case \"the minted cursor\": the walk must resume, got {resumed}"
    );
}
