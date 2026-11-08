// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListFirewallDomainLists`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].
//!
//! What these cases pin is the paging contract of a `@paginated` list operation
//! over the collection this one reports: the domain lists DNS Firewall
//! publishes itself, followed by the records this account created, walked
//! exactly once whatever the page size, with a cursor only while items remain.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member, ACCOUNT, REGION};

/// The domain lists Route 53 Resolver DNS Firewall publishes to every account,
/// in the order this emulator reports them.
///
/// Source: the managed domain lists AWS publishes for DNS Firewall. The model
/// itself names the first of them, with its owner, in
/// `FirewallDomainList$ManagedOwnerName`'s doc
/// (`smithy_query(query="doc",shape="FirewallDomainList",member="ManagedOwnerName")`):
/// "the managed domain list `AWSManagedDomainsMalwareDomainList` has the managed
/// owner name `Route 53 Resolver DNS Firewall`".
const PUBLISHED_LISTS: &[&str] = &[
    "AWSManagedDomainsMalwareDomainList",
    "AWSManagedDomainsBotnetCommandandControl",
    "AWSManagedDomainsAggregateThreatList",
    "AWSManagedDomainsAmazonGuardDutyThreatList",
];

/// The `ManagedOwnerName` every published list reports, from the model doc cited
/// on [`PUBLISHED_LISTS`].
const PUBLISHED_OWNER: &str = "Route 53 Resolver DNS Firewall";

/// The whole `DomainListType` enum
/// (`smithy_query(query="shape",shape="DomainListType")`), which a published
/// list's `ManagedListType` must be one of.
const DOMAIN_LIST_TYPES: &[&str] = &["THREAT", "CONTENT"];

/// The `FirewallDomainLists` of one page.
fn page_items(body: &Value) -> Vec<Value> {
    body.get("FirewallDomainLists")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the response carries a FirewallDomainLists array, got {body}"))
}

/// Walk every page of `ListFirewallDomainLists` for `request`, returning the
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
        let page = ok(router, "ListFirewallDomainLists", body).await;
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

/// Create `count` domain lists and return their ids in creation order.
async fn seed_lists(router: &MultiServiceRouter, count: usize, prefix: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let created = ok(
            router,
            "CreateFirewallDomainList",
            json!({
                "CreatorRequestId": format!("req-{prefix}-{index}"),
                "Name": format!("{prefix}-{index}"),
            }),
        )
        .await;
        let list = created
            .get("FirewallDomainList")
            .expect("the create response carries a FirewallDomainList");
        ids.push(string_member(list, "Id"));
    }
    ids
}

/// An account that has created nothing still sees the lists DNS Firewall
/// publishes, and each one is the same resource on every read.
///
/// The rows are the published catalog itself — including the entries the
/// recorded case never named — and each is checked against what the model says
/// a published list reports: a `ResourceId`-shaped `Id`, an `Arn` rooted in the
/// signed region, a `ManagedOwnerName` (the member used "only for lists that are
/// not managed by you") and a `ManagedListType` out of the model's enum.
#[tokio::test]
async fn the_published_lists_are_visible_without_any_create() {
    let router = emulator();

    let first = walk(&router, &json!({}), "a store nobody wrote to").await;
    let names: Vec<String> = first
        .iter()
        .map(|item| string_member(item, "Name"))
        .collect();
    assert_eq!(
        names,
        PUBLISHED_LISTS.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
        "case \"a store nobody wrote to\": the listing must report exactly the published lists, got {first:?}"
    );

    for item in &first {
        let case = string_member(item, "Name");
        let id = string_member(item, "Id");
        assert!(
            id.starts_with("rslvr-fdl-") && !id.is_empty() && id.len() <= 64,
            "case {case:?}: a domain list Id is rslvr-fdl-<hex> inside ResourceId's length 1..64, got {id:?}"
        );
        assert_eq!(
            string_member(item, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-domain-list/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and this id, got {item}"
        );
        assert_eq!(
            string_member(item, "ManagedOwnerName"),
            PUBLISHED_OWNER,
            "case {case:?}: a list this account does not manage must report its owner, got {item}"
        );
        let managed_type = string_member(item, "ManagedListType");
        assert!(
            DOMAIN_LIST_TYPES.contains(&managed_type.as_str()),
            "case {case:?}: ManagedListType must be one of {DOMAIN_LIST_TYPES:?}, got {managed_type:?}"
        );
        assert_eq!(
            string_member(item, "CreatorRequestId"),
            case,
            "case {case:?}: a published list is identified by the name DNS Firewall publishes it \
             under, got {item}"
        );
        // Every published list says what the intelligence inside it is about.
        assert!(
            !string_member(item, "Category").is_empty(),
            "case {case:?}: a published list must report its Category, got {item}"
        );

        // The id the listing reported resolves to the same list through the
        // reader, so one id names one resource across the service.
        let fetched = ok(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id.clone() }),
        )
        .await;
        let read = fetched
            .get("FirewallDomainList")
            .expect("the read carries a FirewallDomainList");
        for member in [
            "Id",
            "Arn",
            "Name",
            "ManagedOwnerName",
            "ManagedListType",
            "Category",
            "CreatorRequestId",
        ] {
            assert_eq!(
                read.get(member),
                item.get(member),
                "case {case:?}: the read must agree with the listing on {member}, got {fetched}"
            );
        }
    }

    // A list is a pure read: a second walk reports the very same entries, with
    // the very same ids, and a create in between only adds to them.
    let second = walk(&router, &json!({}), "a second walk").await;
    assert_eq!(
        first, second,
        "case \"a second walk\": listing twice must report the same entries"
    );
    let created = seed_lists(&router, 1, "after").await;
    let third = walk(&router, &json!({}), "after a create").await;
    assert_eq!(
        third[..first.len()],
        first[..],
        "case \"after a create\": the published lists must stay where they were, got {third:?}"
    );
    assert_eq!(
        third.len(),
        first.len() + 1,
        "case \"after a create\": the create must add exactly one entry, got {third:?}"
    );
    assert_eq!(
        string_member(&third[first.len()], "Id"),
        created[0],
        "case \"after a create\": the created list must be the entry added, got {third:?}"
    );
}

/// One row per created list: every list this account creates is reported by the
/// listing, with the members the metadata shape declares taken from the record
/// the create wrote.
///
/// The table spans what the rule generalises over — the shape of `Name` (letters,
/// digits beside letters, underscore, hyphen, space, apostrophe, the one- and
/// 64-character bounds) and `Tags` present and absent, none of which the listing
/// reports differently.
#[tokio::test]
async fn every_created_list_is_reported_with_the_members_of_its_record() {
    let longest_name = "n".repeat(64);
    let cases = vec![
        (
            "letters only, no Tags",
            json!({ "CreatorRequestId": "req-letters", "Name": "blocked-domains" }),
        ),
        (
            "digits beside letters and an underscore",
            json!({
                "CreatorRequestId": "req-mixed",
                "Name": "list_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
        ),
        (
            "spaces and an apostrophe",
            json!({ "CreatorRequestId": "req-spaces", "Name": "ops' blocked list" }),
        ),
        (
            "one character",
            json!({ "CreatorRequestId": "req-shortest", "Name": "a", "Tags": [] }),
        ),
        (
            "the longest Name the model allows",
            json!({ "CreatorRequestId": "req-longest", "Name": longest_name }),
        ),
    ];

    let router = emulator();
    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, body) in cases {
        let response = ok(&router, "CreateFirewallDomainList", body).await;
        created.push((
            case,
            response
                .get("FirewallDomainList")
                .expect("the create response carries a FirewallDomainList")
                .clone(),
        ));
    }

    let listed = walk(&router, &json!({}), "every created list").await;
    let own: Vec<Value> = listed
        .iter()
        .filter(|item| item.get("ManagedOwnerName").is_none())
        .cloned()
        .collect();
    assert_eq!(
        own.len(),
        created.len(),
        "case \"every created list\": the listing must report each created list once, got {listed:?}"
    );

    for ((case, made), item) in created.iter().zip(own.iter()) {
        // `FirewallDomainListMetadata` is "minimal high-level information": the
        // members it declares must be the record's, and the members that
        // describe a list this account does NOT manage must be unset.
        for member in ["Id", "Arn", "Name", "CreatorRequestId"] {
            assert_eq!(
                item.get(member),
                made.get(member),
                "case {case:?}: {member} must be the created record's, got {item}"
            );
        }
        for member in ["ManagedOwnerName", "ManagedListType", "Category"] {
            assert_eq!(
                item.get(member),
                None,
                "case {case:?}: {member} must be unset for a list this account created, got {item}"
            );
        }
        // The metadata shape carries no full-list members; those are what
        // `GetFirewallDomainList` is for.
        for member in [
            "DomainCount",
            "Status",
            "StatusMessage",
            "CreationTime",
            "ModificationTime",
        ] {
            assert_eq!(
                item.get(member),
                None,
                "case {case:?}: {member} is not a member of FirewallDomainListMetadata, got {item}"
            );
        }
    }
}

/// Paging walks every domain list exactly once, whatever the page size: the
/// `NextToken` resumes where the previous page stopped, and the last page
/// carries none.
#[tokio::test]
async fn paging_walks_every_domain_list_exactly_once() {
    let router = emulator();
    seed_lists(&router, 5, "paged").await;
    let expected: Vec<String> = walk(&router, &json!({}), "MaxResults omitted")
        .await
        .iter()
        .map(|item| string_member(item, "Id"))
        .collect();
    assert_eq!(
        expected.len(),
        PUBLISHED_LISTS.len() + 5,
        "the collection must be the published lists plus the five created ones, got {expected:?}"
    );

    // Page sizes spanning the modelled MaxResults range (1..100): both published
    // bounds, sizes that do and do not divide the collection, exactly the
    // collection size, and MaxResults omitted altogether.
    for max_results in [Some(1), Some(2), Some(3), Some(4), Some(8), Some(100), None] {
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
            "case {case:?}: paging must walk every domain list exactly once, in the same order"
        );
    }

    // A page that ends the collection carries no cursor, and one that does not
    // carries one — so a caller knows when to stop without a second empty call.
    let first = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 1 }),
    )
    .await;
    assert!(
        first.get("NextToken").is_some(),
        "case \"a page with items left over\": a cursor is owed, got {first}"
    );
    let whole = ok(
        &router,
        "ListFirewallDomainLists",
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
    seed_lists(&router, 2, "bounded").await;

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
            "ListFirewallDomainLists",
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
    seed_lists(&router, 4, "cursor").await;

    let page = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 1 }),
    )
    .await;
    let minted = page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a page with items left over carries a cursor")
        .to_string();

    // Rows spanning the ways a cursor can fail to be this service's: nothing
    // like a cursor, a cursor whose payload was edited, a truncated one, and a
    // plausible plaintext offset nobody minted.
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
            hex::encode("ListFirewallDomainLists/all/v1:2"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallDomainLists",
            json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this service did not mint must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );
    }

    // The cursor this service did mint still resumes the walk, so the refusals
    // above are about the token and not about refusing to resume.
    let resumed = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 1, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        page_items(&resumed).len(),
        1,
        "case \"the minted cursor\": the walk must resume, got {resumed}"
    );
}
