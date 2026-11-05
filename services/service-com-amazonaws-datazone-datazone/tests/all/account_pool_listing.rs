// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAccountPools`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches into
//! `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. The listing reports the account pools the named domain HOLDS, each as the
//!    `AccountPoolSummary` the model declares, agreeing member for member with what
//!    `GetAccountPool` reports for that same pool — so a pool created, or created
//!    elsewhere, is reported exactly where it is stored.
//! 2. A page holds at most `maxResults` summaries in one total order (`sortBy=NAME` by
//!    name, otherwise the pool's id, reversed by `sortOrder=DESCENDING`), and following
//!    the `nextToken` visits every pool exactly once, in that order.
//! 3. `name` selects the pools stored under exactly that name — not a prefix, not
//!    another case — and two pools sharing a name are both reported.
//! 4. A domain this emulator does not hold, and a `nextToken` it never issued, are
//!    refused with the modelled validation error.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};
// The one `ResolutionStrategy` member as the service stores it, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::RESOLUTION_STRATEGY_MANUAL;

/// One account pool to create: its `name` is what the `name` filter and the `NAME` order
/// are judged on, and `handler_source` picks which member of the `AccountSource` union
/// the pool is created with — the summary must not depend on it.
struct PoolRow {
    name: String,
    handler_source: bool,
}

/// The pools every listing case is asserted over, created in an order that is NOT their
/// name order, so an ordering assertion cannot pass by accident.
///
/// They span what the rule generalises over: `AccountPoolName`'s whole character class
/// and both its length bounds (`^[\w -]+$`, 1..64 — SOURCE:
/// `smithy_query(query="shape", shape="AccountPoolName")`), namely a one-character name,
/// a 64-character name, digits, spaces, an underscore, a hyphen, upper and lower case,
/// and TWO pools sharing one name.
fn pool_rows() -> Vec<PoolRow> {
    [
        "z",
        "beta_pool",
        "duplicate name",
        "Alpha pool",
        "0123456789",
        "duplicate name",
        "beta-pool",
        &"n".repeat(64),
        "Beta pool",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, name)| PoolRow {
        name: name.to_string(),
        handler_source: index % 3 == 0,
    })
    .collect()
}

impl PoolRow {
    /// The `CreateAccountPool` body for this row.
    fn body(&self) -> Value {
        let account_source = if self.handler_source {
            json!({
                "customAccountPoolHandler": {
                    "lambdaFunctionArn":
                        "arn:aws:lambda:us-west-2:111122223333:function:resolve-accounts",
                }
            })
        } else {
            json!({
                "accounts": [
                    { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                ]
            })
        };
        json!({
            "name": self.name,
            "resolutionStrategy": RESOLUTION_STRATEGY_MANUAL,
            "accountSource": account_source,
        })
    }
}

/// One pool as it was stored: the name it was created with and the id the service minted.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredPool {
    name: String,
    id: String,
}

/// Create every row of [`pool_rows`] in `domain`, answering what was stored.
async fn create_pools(dz: &Emulator, domain: &str) -> Vec<StoredPool> {
    let mut stored = Vec::new();
    for row in pool_rows() {
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/account-pools"),
                Some(row.body()),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "creating pool {:?}: expected 201, got {} {:?}",
            row.name, created.status, created.body
        );
        stored.push(StoredPool {
            name: row.name.clone(),
            id: created.s("id"),
        });
    }
    stored
}

/// The `(name, id)` pairs one listing answer reports, in the order it reported them.
fn reported(answer: &crate::harness::Answer) -> Vec<StoredPool> {
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| StoredPool {
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("<absent>")
                .to_string(),
            id: item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<absent>")
                .to_string(),
        })
        .collect()
}

/// `pools` in the order a listing sorted by `NAME` must report them: by name, then by id,
/// so two pools sharing a name still have one stable position each.
fn in_name_order(mut pools: Vec<StoredPool>) -> Vec<StoredPool> {
    pools.sort_by(|left, right| (&left.name, &left.id).cmp(&(&right.name, &right.id)));
    pools
}

/// `pools` in the order a listing naming no `sortBy` must report them: by the pool's own
/// id, which is what the store keys them under.
fn in_id_order(mut pools: Vec<StoredPool>) -> Vec<StoredPool> {
    pools.sort_by(|left, right| left.id.cmp(&right.id));
    pools
}

/// Percent-encode a query value, so a name carrying a space or any other byte outside
/// the unreserved set survives the URI.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Walk every page of one listing query, asserting no page exceeds `page_size` and the
/// walk terminates, and answer the pools it visited in order.
async fn page_walk(
    dz: &Emulator,
    domain: &str,
    query: &str,
    page_size: usize,
    expected_len: usize,
) -> Vec<StoredPool> {
    let mut seen: Vec<StoredPool> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut uri = format!("/v2/domains/{domain}/account-pools?maxResults={page_size}{query}");
        if let Some(ref next) = token {
            uri.push_str(&format!("&nextToken={}", encode(next)));
        }
        let answer = dz.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}/{page_size}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let page = reported(&answer);
        assert!(
            page.len() <= page_size,
            "case {query:?}/{page_size}: a page must not exceed maxResults, got {page:?}"
        );
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= expected_len + 1,
            "case {query:?}/{page_size}: the page walk does not terminate, saw {seen:?}"
        );
        match answer.body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// Every pool of one domain in one call: the modelled `maxResults` ceiling is 50, which
/// is more than any case here stores.
async fn list_all(dz: &Emulator, domain: &str, query: &str) -> crate::harness::Answer {
    let answer = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/account-pools{query}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "listing {domain} {query:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.get("nextToken").is_none(),
        "listing {domain} {query:?}: every pool fits one page, so no token is owed, got {:?}",
        answer.body
    );
    answer
}

#[tokio::test]
async fn the_listing_reports_the_pools_the_domain_holds_as_the_modelled_summary() {
    let dz = Emulator::new();
    let domain = dz.available_domain("pool-listing-domain").await;
    let elsewhere = dz.available_domain("pool-listing-elsewhere").await;

    // A domain that holds no pool reports an empty listing, not a refusal.
    assert_eq!(
        reported(&list_all(&dz, &domain, "").await),
        Vec::new(),
        "a domain with no account pool must report an empty listing"
    );

    let stored = create_pools(&dz, &domain).await;
    // The SAME names in another domain: a pool's namespace is its domain, so neither
    // listing may report the other's pools.
    let stored_elsewhere = create_pools(&dz, &elsewhere).await;

    for (label, where_domain, expected) in [
        ("the domain under test", &domain, &stored),
        ("another domain", &elsewhere, &stored_elsewhere),
    ] {
        let listed = reported(&list_all(&dz, where_domain, "").await);
        assert_eq!(
            listed,
            in_id_order(expected.clone()),
            "case {label:?}: the listing must report exactly the pools that domain holds"
        );
    }

    // Each summary agrees member for member with what `GetAccountPool` reports for that
    // pool, and carries only the members `AccountPoolSummary` declares (SOURCE:
    // smithy_query(query="shape", shape="AccountPoolSummary")).
    let listed = list_all(&dz, &domain, "").await;
    let items = listed
        .body
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in &items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("<absent>");
        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/account-pools/{id}"),
                None,
            )
            .await;
        assert_eq!(
            got.status, 200,
            "pool {id:?}: expected 200 from GetAccountPool, got {} {:?}",
            got.status, got.body
        );
        for member in [
            "domainId",
            "id",
            "name",
            "resolutionStrategy",
            "domainUnitId",
            "createdBy",
        ] {
            assert_eq!(
                item.get(member),
                got.body.get(member),
                "pool {id:?}: the summary's {member} must be the stored one GetAccountPool reports, got {:?}",
                item.get(member)
            );
        }
        assert_eq!(
            item.get("domainId").and_then(Value::as_str),
            Some(domain.as_str()),
            "pool {id:?}: the summary must report its own domain, got {:?}",
            item.get("domainId")
        );
        // Nothing has updated these pools, so no principal is reported as their updater.
        assert!(
            item.get("updatedBy").is_none(),
            "pool {id:?}: a pool nothing updated must report no updatedBy, got {item:?}"
        );
        // Members the summary shape does not declare are not invented for it.
        for absent in ["description", "accountSource", "createdAt", "lastUpdatedAt"] {
            assert!(
                item.get(absent).is_none(),
                "pool {id:?}: AccountPoolSummary declares no {absent}, got {item:?}"
            );
        }
    }

    // The listing reads the store: a pool created after it appears in the next one, and
    // deleting the domain takes its pools out of reach.
    let extra = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/account-pools"),
            Some(json!({
                "name": "added after the listing",
                "resolutionStrategy": RESOLUTION_STRATEGY_MANUAL,
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "999988887777", "supportedRegions": ["eu-west-1"] }
                    ]
                },
            })),
        )
        .await;
    assert_eq!(
        extra.status, 201,
        "creating one more pool: expected 201, got {} {:?}",
        extra.status, extra.body
    );
    let mut with_extra = stored.clone();
    with_extra.push(StoredPool {
        name: "added after the listing".to_string(),
        id: extra.s("id"),
    });
    assert_eq!(
        reported(&list_all(&dz, &domain, "").await),
        in_id_order(with_extra),
        "a pool created after a listing must be reported by the next one"
    );
}

#[tokio::test]
async fn a_page_walk_visits_every_pool_exactly_once_in_the_requested_order() {
    let dz = Emulator::new();
    let domain = dz.available_domain("paged-pools-domain").await;
    let stored = create_pools(&dz, &domain).await;

    let by_name = in_name_order(stored.clone());
    let by_name_descending: Vec<StoredPool> = by_name.iter().rev().cloned().collect();
    let by_id = in_id_order(stored.clone());
    let by_id_descending: Vec<StoredPool> = by_id.iter().rev().cloned().collect();

    // The orders the two sort members span: `SortFieldAccountPool` is NAME only and
    // `SortOrder` is ASCENDING | DESCENDING (SOURCE:
    // smithy_query(query="shape", shape="SortFieldAccountPool,SortOrder")), and a request
    // naming neither is answered in the pools' own id order.
    let orders: Vec<(&str, &Vec<StoredPool>)> = vec![
        ("", &by_id),
        ("&sortOrder=ASCENDING", &by_id),
        ("&sortOrder=DESCENDING", &by_id_descending),
        ("&sortBy=NAME", &by_name),
        ("&sortBy=NAME&sortOrder=ASCENDING", &by_name),
        ("&sortBy=NAME&sortOrder=DESCENDING", &by_name_descending),
    ];

    // `MaxResults` is `@range` min 1, max 50 (SOURCE: smithy_query(query="shape",
    // shape="MaxResults")): every page size inside the range, both published bounds
    // included, walks the same sequence.
    for page_size in [1_usize, 2, 4, stored.len(), 50] {
        for (query, expected) in &orders {
            let seen = page_walk(&dz, &domain, query, page_size, expected.len()).await;
            assert_eq!(
                &seen,
                *expected,
                "case {query:?}/{page_size}: the page walk must visit every pool exactly once, in order"
            );
        }
    }

    // One past each published `maxResults` bound is refused by the modelled range.
    for out_of_range in ["0", "51"] {
        let answer = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/account-pools?maxResults={out_of_range}"),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults={out_of_range}: expected the modelled range to refuse it, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn the_name_filter_selects_the_pools_stored_under_exactly_that_name() {
    let dz = Emulator::new();
    let domain = dz.available_domain("pool-name-filter-domain").await;
    let stored = create_pools(&dz, &domain).await;

    // Each distinct stored name: the filter answers every pool stored under it, which is
    // two pools for the name this table repeats.
    let mut names: Vec<String> = stored.iter().map(|pool| pool.name.clone()).collect();
    names.sort();
    names.dedup();
    for name in &names {
        let expected: Vec<StoredPool> = in_name_order(
            stored
                .iter()
                .filter(|pool| &pool.name == name)
                .cloned()
                .collect(),
        );
        let answer = list_all(&dz, &domain, &format!("?name={}&sortBy=NAME", encode(name))).await;
        assert_eq!(
            reported(&answer),
            expected,
            "case name={name:?}: the filter must report every pool stored under that name"
        );

        // The same filter, paged one pool at a time: filtering and paging compose.
        let seen = page_walk(
            &dz,
            &domain,
            &format!("&name={}&sortBy=NAME", encode(name)),
            1,
            expected.len(),
        )
        .await;
        assert_eq!(
            seen, expected,
            "case name={name:?}, one pool per page: the walk must visit the same pools, in order"
        );
    }

    // A name no pool is stored under selects nothing: the filter is exact, so neither a
    // prefix of a stored name, nor another case of one, nor a name nothing created
    // matches.
    for miss in ["beta", "BETA_POOL", "Duplicate Name", "nosuchpool", "z z"] {
        let answer = list_all(&dz, &domain, &format!("?name={}", encode(miss))).await;
        assert_eq!(
            reported(&answer),
            Vec::new(),
            "case name={miss:?}: no pool is stored under that name, so nothing is reported"
        );
    }
}

#[tokio::test]
async fn an_unknown_domain_or_a_token_this_service_never_issued_is_refused() {
    let dz = Emulator::new();
    let domain = dz.available_domain("pool-listing-refusals").await;
    create_pools(&dz, &domain).await;

    // A permission is held ON a domain, so a caller holds none on an identifier this
    // store serves no domain for: authorization decides the listing before any lookup
    // (403 `AccessDeniedException`). Inside a domain it may list, a token this service
    // never issued is still invalid input — this operation models no
    // `ResourceNotFoundException` (SOURCE: smithy_query(query="operation",
    // operation="ListAccountPools")).
    let denied = dz
        .call("GET", "/v2/domains/dzd_nosuchdomain000/account-pools", None)
        .await;
    assert_eq!(
        (denied.status, denied.error_code.as_str(), denied.s("message")),
        (
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: ListAccountPools".to_string()
        ),
        "a listing scoped to a domain nothing created must be denied, got {} {:?}",
        denied.status,
        denied.body
    );
    let cases: Vec<(&str, String)> = vec![
        (
            "a token this service never issued",
            format!("/v2/domains/{domain}/account-pools?nextToken=notatoken"),
        ),
        (
            "a token that is not even hex",
            format!("/v2/domains/{domain}/account-pools?nextToken=zzzz"),
        ),
    ];
    for (label, uri) in cases {
        let answer = dz.call("GET", &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // A domain on its way out holds no pools a caller may list: while it is still
    // stored the listing refuses exactly where `GetAccountPool` reports nothing, and
    // once the clock has taken it away the identifier names no domain at all, so
    // authorization denies the call.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    for (label, status, code) in [
        ("while the domain is being deleted", 400, "ValidationException"),
        ("once it is gone", 403, "AccessDeniedException"),
    ] {
        let answer = dz
            .call("GET", &format!("/v2/domains/{domain}/account-pools"), None)
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
        // Wait out the chosen lifecycle span so the second pass runs after the domain
        // has gone.
        tokio::time::sleep(SETTLED).await;
    }
}
