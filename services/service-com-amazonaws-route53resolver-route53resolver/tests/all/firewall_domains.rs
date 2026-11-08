// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DNS Firewall domain slice: `UpdateFirewallDomains`
//! and the reader its writes are read back through, `ListFirewallDomains`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! What these cases pin is the rule the operation implements: a domain list
//! holds a SET of domains, each stored as the DNS-canonical, fully qualified
//! name — rooted with one trailing dot, whatever spelling the request wrote —
//! membership decided on the case-folded canonical name, and one update applies
//! exactly one set operation to it — `ADD` the union, `REMOVE` the difference,
//! `REPLACE` the requested set itself — which every later read of the list
//! reports.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    firewall_domain_canonical, MAX_DOMAINS_PER_UPDATE, PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, expect_error, ok, parse_time, string_member};

/// The `FirewallDomainListStatus` values the model declares
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus")`), which any
/// status a response reports must be one of.
const DOMAIN_LIST_STATUSES: &[&str] = &[
    "COMPLETE",
    "COMPLETE_IMPORT_FAILED",
    "IMPORTING",
    "DELETING",
    "UPDATING",
];

/// One domain list DNS Firewall publishes itself, named in
/// `FirewallDomainList$ManagedOwnerName`'s doc
/// (`smithy_query(query="doc",shape="FirewallDomainList",member="ManagedOwnerName")`).
const PUBLISHED_LIST: &str = "AWSManagedDomainsMalwareDomainList";

/// Create a domain list and return its id.
async fn seed_list(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("req-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create response carries a FirewallDomainList"),
        "Id",
    )
}

/// Apply one `UpdateFirewallDomains` call that must succeed.
async fn update(
    router: &MultiServiceRouter,
    id: &str,
    operation: &str,
    domains: &[String],
) -> Value {
    ok(
        router,
        "UpdateFirewallDomains",
        json!({
            "FirewallDomainListId": id,
            "Operation": operation,
            "Domains": domains,
        }),
    )
    .await
}

/// The `FirewallDomainList` of a `GetFirewallDomainList` response.
async fn read_list(router: &MultiServiceRouter, id: &str) -> Value {
    let fetched = ok(
        router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": id }),
    )
    .await;
    fetched
        .get("FirewallDomainList")
        .expect("the read carries a FirewallDomainList")
        .clone()
}

/// Walk every page of `ListFirewallDomains` for one list, asserting that no page
/// is longer than `MaxResults` and that paging terminates.
async fn walk_domains(
    router: &MultiServiceRouter,
    id: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut body = json!({ "FirewallDomainListId": id });
        if let Some(size) = max_results {
            body["MaxResults"] = json!(size);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(router, "ListFirewallDomains", body).await;
        pages += 1;
        assert!(
            pages <= 1_000,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        let items: Vec<String> = page
            .get("Domains")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the response carries a Domains array, got {page}"))
            .iter()
            .map(|item| {
                item.as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a domain is a string, got {item}"))
                    .to_string()
            })
            .collect();
        if let Some(size) = max_results {
            assert!(
                i64::try_from(items.len()).unwrap_or(i64::MAX) <= size,
                "case {case:?}: a page holds at most MaxResults domains, got {} in {page}",
                items.len()
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

/// `domains`, sorted, so two collections are compared as the sets they are.
fn as_set(domains: &[String]) -> Vec<String> {
    let mut sorted = domains.to_vec();
    sorted.sort();
    sorted
}

/// `domains` in the spelling a domain list reads them back in: the fully
/// qualified name, rooted with one trailing dot — the canonicalization the
/// write path applies, through the service's own one
/// ([`firewall_domain_canonical`]), so what a case asserts is the stored form of
/// the domains it wrote and not a second copy of the rule.
fn rooted(domains: &[String]) -> Vec<String> {
    domains
        .iter()
        .map(|domain| firewall_domain_canonical(domain))
        .collect()
}

/// Assert the list's domains are exactly `expected`, read back through the
/// reader and counted through `GetFirewallDomainList`.
async fn assert_domains(
    router: &MultiServiceRouter,
    id: &str,
    expected: &[String],
    case: &str,
) {
    let walked = walk_domains(router, id, None, case).await;
    let distinct: HashSet<String> = walked.iter().map(|d| d.to_ascii_lowercase()).collect();
    assert_eq!(
        distinct.len(),
        walked.len(),
        "case {case:?}: a domain list holds each domain once, got {walked:?}"
    );
    let expected = rooted(expected);
    assert_eq!(
        as_set(&walked),
        as_set(&expected),
        "case {case:?}: expected the domains {:?}, got {walked:?}",
        as_set(&expected)
    );
    let read = read_list(router, id).await;
    assert_eq!(
        read.get("DomainCount"),
        Some(&json!(expected.len())),
        "case {case:?}: DomainCount is the number of domains stored, got {read}"
    );
    // A read never changes what a later caller sees: the second walk reports
    // the very same domains, in the very same order.
    let again = walk_domains(router, id, None, case).await;
    assert_eq!(
        again, walked,
        "case {case:?}: listing twice must report the same domains in the same order"
    );
}

fn owned(domains: &[&str]) -> Vec<String> {
    domains.iter().map(|d| (*d).to_string()).collect()
}

/// One row of the set-operation table: the case name, the domains the list
/// holds to begin with, the `Operation` applied, the `Domains` it carries and
/// the domains the list must hold afterwards.
type SetOperationCase<'a> = (
    &'a str,
    Vec<&'a str>,
    &'a str,
    Vec<&'a str>,
    Vec<&'a str>,
);

/// The longest domain the model's `FirewallDomainName` admits (length 1..255),
/// written as DNS labels within the 63 octets RFC 1035 §2.3.4 allows: four
/// 63-character labels joined by dots is exactly 255 characters.
fn longest_domain() -> String {
    let label = "a".repeat(63);
    [label.clone(), label.clone(), label.clone(), label].join(".")
}

/// One row per way a set operation can meet a stored list: the operation is
/// applied to the domains the list already holds and nothing else changes.
///
/// The rows span what the rule generalises over — which operation, how the
/// requested domains overlap the stored ones (disjoint, overlapping, equal,
/// absent), the case they are written in, how many there are (none, one, many),
/// and the shape of a domain (a wildcard, an underscored label, a single label,
/// the longest the model allows).
#[tokio::test]
async fn one_update_applies_exactly_one_set_operation_to_the_stored_domains() {
    let longest = longest_domain();
    let cases: Vec<SetOperationCase> = vec![
        ("ADD into a list created empty", vec![], "ADD", vec!["example.com"], vec!["example.com"]),
        (
            "ADD domains beside the stored ones",
            vec!["a.example.com", "b.example.com"],
            "ADD",
            vec!["c.example.com", "d.example.com"],
            vec!["a.example.com", "b.example.com", "c.example.com", "d.example.com"],
        ),
        (
            "ADD a domain the list already holds",
            vec!["a.example.com", "b.example.com"],
            "ADD",
            vec!["b.example.com", "c.example.com"],
            vec!["a.example.com", "b.example.com", "c.example.com"],
        ),
        (
            "ADD a stored domain written in another case",
            vec!["a.example.com"],
            "ADD",
            vec!["A.Example.COM"],
            vec!["a.example.com"],
        ),
        (
            "ADD the same domain twice in one request",
            vec![],
            "ADD",
            vec!["dup.example.com", "dup.example.com"],
            vec!["dup.example.com"],
        ),
        (
            "ADD a wildcard, an underscored label and a single label",
            vec![],
            "ADD",
            vec!["*.example.com", "_dmarc.example.com", "localhost"],
            vec!["*.example.com", "_dmarc.example.com", "localhost"],
        ),
        (
            "ADD nothing",
            vec!["a.example.com"],
            "ADD",
            vec![],
            vec!["a.example.com"],
        ),
        (
            "REMOVE a subset of the stored domains",
            vec!["a.example.com", "b.example.com", "c.example.com"],
            "REMOVE",
            vec!["b.example.com"],
            vec!["a.example.com", "c.example.com"],
        ),
        (
            "REMOVE a domain the list does not hold",
            vec!["a.example.com"],
            "REMOVE",
            vec!["z.example.com"],
            vec!["a.example.com"],
        ),
        (
            "REMOVE a stored domain written in another case",
            vec!["a.example.com", "b.example.com"],
            "REMOVE",
            vec!["A.EXAMPLE.COM"],
            vec!["b.example.com"],
        ),
        (
            "REMOVE every stored domain",
            vec!["a.example.com", "b.example.com"],
            "REMOVE",
            vec!["a.example.com", "b.example.com"],
            vec![],
        ),
        (
            "REPLACE with a disjoint set",
            vec!["old-one.example.com", "old-two.example.com"],
            "REPLACE",
            vec!["new.example.com"],
            vec!["new.example.com"],
        ),
        (
            "REPLACE with a set that overlaps the stored one",
            vec!["a.example.com", "b.example.com"],
            "REPLACE",
            vec!["b.example.com", "c.example.com"],
            vec!["b.example.com", "c.example.com"],
        ),
        (
            "REPLACE with nothing",
            vec!["a.example.com", "b.example.com"],
            "REPLACE",
            vec![],
            vec![],
        ),
        (
            "REPLACE with the longest domain the model allows",
            vec!["a.example.com"],
            "REPLACE",
            vec![longest.as_str()],
            vec![longest.as_str()],
        ),
    ];

    let router = emulator();
    for (index, (case, initial, operation, requested, expected)) in cases.into_iter().enumerate() {
        let name = format!("case-{index}");
        let id = seed_list(&router, &name).await;

        // The list starts out holding exactly `initial`, put there through the
        // operation that makes a list "exactly match the list you provide".
        if !initial.is_empty() {
            update(&router, &id, "REPLACE", &owned(&initial)).await;
            assert_domains(&router, &id, &owned(&initial), &format!("{case} (seeded)")).await;
        }

        let answered = update(&router, &id, operation, &owned(&requested)).await;
        assert_eq!(
            string_member(&answered, "Id"),
            id,
            "case {case:?}: the update reports the list it updated, got {answered}"
        );
        assert_eq!(
            string_member(&answered, "Name"),
            name,
            "case {case:?}: the update reports the list's Name, got {answered}"
        );
        let status = string_member(&answered, "Status");
        assert!(
            DOMAIN_LIST_STATUSES.contains(&status.as_str()),
            "case {case:?}: Status must be one of {DOMAIN_LIST_STATUSES:?}, got {status:?}"
        );
        assert_eq!(
            status, "COMPLETE",
            "case {case:?}: a set operation applied inside the call leaves the list settled, got {answered}"
        );

        assert_domains(&router, &id, &owned(&expected), case).await;

        // The write is dated: a list someone updated reports a
        // ModificationTime no earlier than its CreationTime.
        let read = read_list(&router, &id).await;
        let created_at = parse_time(case, "CreationTime", &string_member(&read, "CreationTime"));
        let modified_at = parse_time(
            case,
            "ModificationTime",
            &string_member(&read, "ModificationTime"),
        );
        assert!(
            modified_at >= created_at,
            "case {case:?}: ModificationTime must not precede CreationTime, got {read}"
        );
    }
}

/// The update is applied inside the call it was made in: the list reports the
/// settled status straight away and still reports it once the span the clock
/// owes a change that IS in flight has passed, with the domains it wrote
/// untouched.
#[tokio::test]
async fn the_update_is_settled_by_the_time_it_answers() {
    let router = emulator();
    let id = seed_list(&router, "settling").await;
    let domains = owned(&["a.example.com", "b.example.com"]);

    let answered = update(&router, &id, "ADD", &domains).await;
    assert_eq!(
        string_member(&answered, "Status"),
        "COMPLETE",
        "case \"straight after the update\": the change is applied inside the call, got {answered}"
    );
    let read = read_list(&router, &id).await;
    assert_eq!(
        string_member(&read, "Status"),
        "COMPLETE",
        "case \"straight after the update\": the list reports the same status as the update, got {read}"
    );
    assert_domains(&router, &id, &domains, "straight after the update").await;

    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let later = read_list(&router, &id).await;
    assert_eq!(
        string_member(&later, "Status"),
        "COMPLETE",
        "case \"a span later\": a settled list stays settled, got {later}"
    );
    assert_domains(&router, &id, &domains, "a span later").await;
}

/// Paging walks every domain of the list exactly once, whatever the page size,
/// and no two pages report the same domain.
#[tokio::test]
async fn paging_walks_every_domain_exactly_once() {
    let router = emulator();
    let id = seed_list(&router, "paged").await;
    let seeded: Vec<String> = (0..250)
        .map(|index| format!("d{index}.example.com"))
        .collect();
    update(&router, &id, "ADD", &seeded).await;

    let expected = as_set(&rooted(&seeded));
    // Page sizes spanning the modelled `ListDomainMaxResults` range (1..5000):
    // both published bounds, sizes that do and do not divide the collection,
    // exactly the collection size, and MaxResults omitted altogether.
    for max_results in [
        Some(1),
        Some(2),
        Some(7),
        Some(99),
        Some(100),
        Some(250),
        Some(5_000),
        None,
    ] {
        let case = format!("MaxResults={max_results:?}");
        let walked = walk_domains(&router, &id, max_results, &case).await;
        let distinct: HashSet<&String> = walked.iter().collect();
        assert_eq!(
            distinct.len(),
            walked.len(),
            "case {case:?}: no two pages may report the same domain, got {} entries and {} distinct",
            walked.len(),
            distinct.len()
        );
        assert_eq!(
            as_set(&walked),
            expected,
            "case {case:?}: paging must walk every domain exactly once"
        );
    }

    // A page that ends the collection carries no cursor, and one that does not
    // carries one — so a caller knows when to stop without a second empty call.
    let first = ok(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id.clone(), "MaxResults": 1 }),
    )
    .await;
    assert!(
        first.get("NextToken").is_some(),
        "case \"a page with domains left over\": a cursor is owed, got {first}"
    );
    let whole = ok(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id, "MaxResults": 5_000 }),
    )
    .await;
    assert_eq!(
        whole.get("NextToken"),
        None,
        "case \"a page that ends the collection\": no cursor is owed, got {whole}"
    );
}

/// `MaxResults` is modelled `range 1..5000`, so a request outside it is refused
/// rather than clamped into a page the caller did not ask for.
///
/// The refusal is the boundary validator's, so only its HTTP status is asserted.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    let id = seed_list(&router, "bounded").await;
    update(&router, &id, "ADD", &owned(&["a.example.com"])).await;

    for (case, max_results, accepted) in [
        ("one below the published minimum", 0, false),
        ("far below the published minimum", -1, false),
        ("the published minimum", 1, true),
        ("the published maximum", 5_000, true),
        ("one above the published maximum", 5_001, false),
        ("far above the published maximum", 100_000, false),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallDomains",
            json!({ "FirewallDomainListId": id.clone(), "MaxResults": max_results }),
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: MaxResults={max_results} must answer HTTP {expected}, got {status}: {body}"
        );
    }
}

/// A `NextToken` this service did not mint for THIS list's domains is refused
/// with the `ValidationException` the operation models, rather than silently
/// read as "start over" and answering page one again.
#[tokio::test]
async fn a_next_token_this_list_did_not_mint_is_refused() {
    let router = emulator();
    let id = seed_list(&router, "cursor").await;
    let other = seed_list(&router, "cursor-other").await;
    let domains: Vec<String> = (0..4).map(|index| format!("c{index}.example.com")).collect();
    update(&router, &id, "ADD", &domains).await;
    update(&router, &other, "ADD", &domains).await;

    let page = ok(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id.clone(), "MaxResults": 1 }),
    )
    .await;
    let minted = page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a page with domains left over carries a cursor")
        .to_string();
    let minted_for_other = ok(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": other, "MaxResults": 1 }),
    )
    .await
    .get("NextToken")
    .and_then(Value::as_str)
    .expect("a page with domains left over carries a cursor")
    .to_string();

    // Rows spanning the ways a cursor can fail to be this collection's: nothing
    // like a cursor, a cursor whose payload was edited, a truncated one, a
    // plausible plaintext offset nobody minted, and a cursor this service did
    // mint — for another domain list.
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
            hex::encode(format!("ListFirewallDomains/v1/{id}:2")),
        ),
        ("a cursor minted for another domain list", minted_for_other),
    ] {
        let (status, body) = call(
            &router,
            "ListFirewallDomains",
            json!({ "FirewallDomainListId": id.clone(), "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this collection did not mint must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ValidationException"),
            "case {case:?}: the refusal must name ValidationException, got {body}"
        );
    }

    // The cursor this collection did mint still resumes the walk, so the
    // refusals above are about the token and not about refusing to resume.
    let resumed = ok(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": id, "MaxResults": 1, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        resumed
            .get("Domains")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case \"the minted cursor\": the walk must resume, got {resumed}"
    );
}

/// A `FirewallDomainListId` no list is stored under is reported as missing by
/// both the write and the read, rather than creating one or answering an empty
/// collection.
#[tokio::test]
async fn an_unknown_domain_list_id_is_reported_as_missing() {
    let router = emulator();
    let stored = seed_list(&router, "present").await;

    for (case, id) in [
        ("an id nothing was ever stored under", "rslvr-fdl-0000000000000000".to_string()),
        ("a stored id with a character changed", format!("{stored}x")),
        ("an id of another resource kind", "rslvr-out-0000000000000000".to_string()),
    ] {
        expect_error(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "ADD",
                "Domains": ["example.com"],
            }),
            case,
            "ResourceNotFoundException",
        )
        .await;
        expect_error(
            &router,
            "ListFirewallDomains",
            json!({ "FirewallDomainListId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // Nothing above wrote anything: the stored list is still the empty one the
    // create made.
    assert_domains(&router, &stored, &[], "the stored list after the refusals").await;
}

/// A domain list DNS Firewall publishes itself belongs to the service, not to
/// the account reading it: it is not visible as a writable resource at all, so
/// a write naming one is answered as a write naming a list that does not exist,
/// and the domains inside it are not this account's to read either.
#[tokio::test]
async fn a_published_domain_list_is_not_this_accounts_to_update() {
    let router = emulator();
    let published = ok(&router, "ListFirewallDomainLists", json!({ "MaxResults": 100 }))
        .await
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the published lists")
        .iter()
        .find(|item| string_member(item, "Name") == PUBLISHED_LIST)
        .map(|item| string_member(item, "Id"))
        .expect("the published malware list is reported");

    expect_error(
        &router,
        "UpdateFirewallDomains",
        json!({
            "FirewallDomainListId": published.clone(),
            "Operation": "ADD",
            "Domains": ["example.com"],
        }),
        "a published list",
        "ResourceNotFoundException",
    )
    .await;

    expect_error(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": published }),
        "a published list",
        "AccessDeniedException",
    )
    .await;
}

/// `Domains`' doc states the requirements a domain specification must satisfy,
/// so one that does not is refused with the `ValidationException` the operation
/// models and the stored list is left alone.
///
/// The accepted rows are the forms the same doc admits — the optional leading
/// asterisk and the dot-separated labels its own wildcard form is written with.
#[tokio::test]
async fn a_domain_outside_the_documented_form_is_refused() {
    let router = emulator();
    let id = seed_list(&router, "forms").await;

    let longest = longest_domain();
    for (case, domain, accepted) in [
        ("a plain domain", "example.com", true),
        ("the optional leading asterisk", "*.example.com", true),
        ("an asterisk without a separator", "*example.com", true),
        ("a single label", "localhost", true),
        ("hyphens and underscores", "a-b_c.example.com", true),
        ("the root label as a trailing dot", "example.com.", true),
        ("the longest domain the model allows", longest.as_str(), true),
        ("a space inside the name", "exa mple.com", false),
        ("a leading space", " example.com", false),
        ("a character the doc excludes", "exa/mple.com", false),
        ("an at sign", "user@example.com", false),
        ("an empty label", "a..example.com", false),
        ("a leading separator", ".example.com", false),
        ("a label past the 63 octets DNS allows", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.example.com", false),
    ] {
        let (status, body) = call(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "ADD",
                "Domains": [domain],
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {domain:?} is a domain specification the doc admits, got {status}: {body}"
            );
            update(&router, &id, "REMOVE", &owned(&[domain])).await;
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {domain:?} must be refused, got {status}: {body}"
            );
            assert!(
                error_type(&body).contains("ValidationException"),
                "case {case:?}: the refusal must name ValidationException, got {body}"
            );
        }
    }

    // Every refusal above wrote nothing, and every accepted domain was removed
    // again, so the list is back to the empty one the create made.
    assert_domains(&router, &id, &[], "after the form cases").await;
}

/// An `Operation` outside the modelled `FirewallDomainUpdateOperation` enum is
/// refused: "supported values are ADD, REMOVE, or REPLACE".
#[tokio::test]
async fn an_operation_outside_the_modelled_enum_is_refused() {
    let router = emulator();
    let id = seed_list(&router, "operations").await;

    for case in ["", "add", "DELETE", "ADD "] {
        let (status, body) = call(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": case,
                "Domains": ["example.com"],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an Operation outside the enum must be refused, got {status}: {body}"
        );
    }

    assert_domains(&router, &id, &[], "after the operation cases").await;
}

/// "There is a limit of 1000 domains per request": a request at the limit is
/// served and one past it is refused with the `LimitExceededException` the
/// operation models, without writing any of its domains.
#[tokio::test]
async fn more_domains_than_the_documented_limit_per_request_is_refused() {
    let router = emulator();
    let id = seed_list(&router, "limit").await;

    let at_limit: Vec<String> = (0..MAX_DOMAINS_PER_UPDATE)
        .map(|index| format!("l{index}.example.com"))
        .collect();
    update(&router, &id, "REPLACE", &at_limit).await;
    assert_domains(&router, &id, &at_limit, "exactly the documented limit").await;

    for (case, count) in [
        ("one past the documented limit", MAX_DOMAINS_PER_UPDATE + 1),
        ("far past the documented limit", MAX_DOMAINS_PER_UPDATE * 2),
    ] {
        let over: Vec<String> = (0..count)
            .map(|index| format!("over{index}.example.com"))
            .collect();
        let (status, body) = call(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "ADD",
                "Domains": over,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {count} domains must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("LimitExceededException"),
            "case {case:?}: the refusal must name LimitExceededException, got {body}"
        );
    }

    // The refused requests wrote nothing: the list still holds exactly the
    // domains the accepted one put there.
    assert_domains(&router, &id, &at_limit, "after the refused requests").await;
}
