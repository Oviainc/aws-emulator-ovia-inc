// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListDomainUnitsForParent`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here. The
//! harness (`Emulator`, `Answer`) is the one the other suites already define.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. The listing reports exactly the stored units whose parent is the unit the request
//!    names — one summary (`id`, `name`) per DIRECT child, never a grandchild, never the
//!    parent itself — and it reports every one of them whatever the subtree's shape.
//! 2. A page holds at most `maxResults` summaries, in one stable order (the store's own
//!    id order), and following `nextToken` walks the children exactly once: the pages
//!    concatenated are the whole child set, with no duplicate and nothing missing, for
//!    every page size in the modelled range.
//! 3. A reporting call changes nothing a later caller sees: listing twice, and listing
//!    page by page, leave every child exactly as `GetDomainUnit` reports it.
//! 4. A request naming a domain or a parent that is not stored, a `nextToken` this
//!    service never issued, or a `maxResults` outside the modelled range, is refused
//!    with a 400 `ValidationException`.

use crate::domain_project_lifecycle::is_domain_unit_id;
use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};

/// The `(id, name)` of every `items` entry, in the order the listing reported them
/// (smithy_query shape="DomainUnitSummary": required `id` and `name`).
fn reported(answer: &Answer) -> Vec<(String, String)> {
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let member = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or("<absent>")
                            .to_string()
                    };
                    (member("id"), member("name"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Create a domain unit under `parent`, asserting the create itself succeeded, and
/// answer its id.
async fn create_unit(dz: &Emulator, domain: &str, parent: &str, name: &str, label: &str) -> String {
    let answer = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({ "name": name, "parentDomainUnitIdentifier": parent })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {label:?}: expected 201 creating {name:?} under {parent:?}, got {} {:?}",
        answer.status, answer.body
    );
    answer.s("id")
}

/// One `ListDomainUnitsForParent` call.
async fn list(dz: &Emulator, domain: &str, parent: &str, query: &str) -> Answer {
    dz.call(
        "GET",
        &format!("/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={parent}{query}"),
        None,
    )
    .await
}

/// One subtree to build under a parent of its own. The rows differ along what the rule
/// generalises over: how many direct children the parent has, how many children each of
/// those has in turn (which must NOT be reported), and the form and length of the names.
struct SubtreeCase {
    label: &'static str,
    /// The names of the parent's direct children.
    children: Vec<String>,
    /// How many children each of those children gets.
    grandchildren_each: usize,
}

fn subtree_cases() -> Vec<SubtreeCase> {
    vec![
        SubtreeCase {
            label: "a leaf parent: no children at all",
            children: Vec::new(),
            grandchildren_each: 0,
        },
        SubtreeCase {
            label: "one child, itself a leaf",
            children: vec!["sales".to_string()],
            grandchildren_each: 0,
        },
        SubtreeCase {
            label: "one child with a subtree of its own",
            children: vec!["only child".to_string()],
            grandchildren_each: 3,
        },
        SubtreeCase {
            // `DomainUnitName` is len 1..128 with pattern `^[\w -]+$`
            // (smithy_query shape="DomainUnitName"): the minimum, the maximum, and
            // names mixing spaces, digits, hyphens and underscores.
            label: "four children spanning the modelled name range, each with children",
            children: vec![
                "a".to_string(),
                "Sales Unit 42".to_string(),
                "eu-west_sub 1".to_string(),
                "n".repeat(128),
            ],
            grandchildren_each: 2,
        },
        SubtreeCase {
            label: "seven children, a page's worth under the default ceiling",
            children: (0..7).map(|index| format!("child {index}")).collect(),
            grandchildren_each: 1,
        },
    ]
}

#[tokio::test]
async fn the_listing_reports_exactly_the_parents_direct_children() {
    let dz = Emulator::new();
    let domain = dz.available_domain("list-units-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    // Every case gets its own parent under the root, so the root's own listing is the
    // set of those parents — asserted at the end.
    let mut parents: Vec<(String, String)> = Vec::new();

    for case in subtree_cases() {
        let parent_name = format!("parent of {}", parents.len());
        let parent = create_unit(&dz, &domain, &root, &parent_name, case.label).await;
        parents.push((parent.clone(), parent_name));

        let mut expected: Vec<(String, String)> = Vec::new();
        for name in &case.children {
            let child = create_unit(&dz, &domain, &parent, name, case.label).await;
            for index in 0..case.grandchildren_each {
                // A grandchild belongs to its OWN parent's listing, never to this one.
                create_unit(
                    &dz,
                    &domain,
                    &child,
                    &format!("grandchild {index}"),
                    case.label,
                )
                .await;
            }
            expected.push((child, name.clone()));
        }

        let listed = list(&dz, &domain, &parent, "").await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.label, listed.status, listed.body
        );
        let mut got = reported(&listed);
        // RULE 2: the listing has one stable order — the store's id order.
        let mut ascending = got.clone();
        ascending.sort();
        assert_eq!(
            got, ascending,
            "case {:?}: expected the summaries in ascending id order, got {got:?}",
            case.label
        );
        got.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "case {:?}: expected exactly the parent's direct children, got {got:?}",
            case.label
        );
        assert!(
            listed.body.get("nextToken").is_none_or(Value::is_null),
            "case {:?}: {} children fit in one page, so no nextToken is offered, got {:?}",
            case.label,
            expected.len(),
            listed.body.get("nextToken")
        );

        // RULE 1 and 3: each reported summary is the stored unit, as its own reader
        // reports it, and the listing changed nothing.
        for (id, name) in &expected {
            assert!(
                is_domain_unit_id(id),
                "case {:?}: expected an id matching ^[a-z0-9_-]+$, got {id:?}",
                case.label
            );
            let got_unit = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/domain-units/{id}"),
                    None,
                )
                .await;
            assert_eq!(
                (got_unit.s("id"), got_unit.s("name"), got_unit.s("parentDomainUnitId")),
                (id.clone(), name.clone(), parent.clone()),
                "case {:?}: GetDomainUnit must report the listed unit unchanged, got {:?}",
                case.label,
                got_unit.body
            );
        }
        let again = reported(&list(&dz, &domain, &parent, "").await);
        assert_eq!(
            again,
            reported(&listed),
            "case {:?}: a second listing must report the same children, got {again:?}",
            case.label
        );
    }

    // The root unit's children are the per-case parents, and nothing deeper.
    let mut root_children = reported(&list(&dz, &domain, &root, "").await);
    root_children.sort();
    parents.sort();
    assert_eq!(
        root_children, parents,
        "the root unit's listing must be the units created directly under it, got {root_children:?}"
    );
}

#[tokio::test]
async fn following_next_token_walks_every_child_exactly_once() {
    let dz = Emulator::new();
    let domain = dz.available_domain("paged-units-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");
    let parent = create_unit(&dz, &domain, &root, "paged parent", "paging").await;

    // One past the modelled `maxResults` ceiling of 25, so even a request naming no
    // page size has to offer a token.
    const CHILD_COUNT: usize = 26;
    let mut all: Vec<(String, String)> = Vec::new();
    for index in 0..CHILD_COUNT {
        let name = format!("paged child {index}");
        let id = create_unit(&dz, &domain, &parent, &name, "paging").await;
        all.push((id, name));
    }
    all.sort();

    // Both published bounds of `MaxResultsForListDomains` (`@range` 1..25), and sizes
    // in between: a page size that divides the set and ones that do not.
    for page_size in [1usize, 2, 5, 7, 25] {
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(t) => format!("&maxResults={page_size}&nextToken={t}"),
                None => format!("&maxResults={page_size}"),
            };
            let page = list(&dz, &domain, &parent, &query).await;
            assert_eq!(
                page.status, 200,
                "case page_size={page_size} page {pages}: expected 200, got {} {:?}",
                page.status, page.body
            );
            let items = reported(&page);
            assert!(
                items.len() <= page_size,
                "case page_size={page_size} page {pages}: a page must not exceed maxResults, got {} items",
                items.len()
            );
            assert!(
                !items.is_empty(),
                "case page_size={page_size} page {pages}: a token must not lead to an empty page, got {:?}",
                page.body
            );
            walked.extend(items);
            pages += 1;
            assert!(
                pages <= CHILD_COUNT + 1,
                "case page_size={page_size}: the walk did not terminate after {pages} pages"
            );
            match page.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let expected_pages = CHILD_COUNT.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page_size={page_size}: expected {expected_pages} pages for {CHILD_COUNT} children, got {pages}"
        );
        // The walk is a stable total order, so the pages concatenated are the whole
        // child set in ascending id order: no duplicate, nothing missing.
        assert_eq!(
            walked, all,
            "case page_size={page_size}: the pages concatenated must be every child exactly once, got {walked:?}"
        );
    }

    // A request naming no page size gets the modelled ceiling and a token for the rest.
    let defaulted = list(&dz, &domain, &parent, "").await;
    assert_eq!(
        reported(&defaulted).len(),
        25,
        "case \"no maxResults\": expected the modelled ceiling of 25 summaries, got {}",
        reported(&defaulted).len()
    );
    let next = defaulted
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !next.is_empty(),
        "case \"no maxResults\": {CHILD_COUNT} children do not fit one page, so a nextToken is required, got {:?}",
        defaulted.body
    );
    let last = list(&dz, &domain, &parent, &format!("&nextToken={next}")).await;
    assert_eq!(
        reported(&last).len(),
        CHILD_COUNT - 25,
        "case \"no maxResults, second page\": expected the remaining {} summaries, got {:?}",
        CHILD_COUNT - 25,
        last.body
    );
    assert!(
        last.body.get("nextToken").is_none_or(Value::is_null),
        "case \"no maxResults, second page\": the last page offers no token, got {:?}",
        last.body.get("nextToken")
    );
}

#[tokio::test]
async fn an_unknown_domain_parent_token_or_page_size_is_refused() {
    let dz = Emulator::new();
    let domain = dz.available_domain("list-units-refusals").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");
    let child = create_unit(&dz, &domain, &root, "a child", "refusals").await;

    // A listing scoped to a domain the store serves no record of is DENIED, whatever
    // the rest of the request names: a permission is held ON a domain, so the caller
    // holds none there and authorization decides the call before any lookup.
    for (label, uri) in [
        (
            "a domain that is not stored",
            format!("/v2/domains/dzd_nosuchdomain000/domain-units?parentDomainUnitIdentifier={root}"),
        ),
        (
            "a parent unit stored in ANOTHER domain",
            format!("/v2/domains/dzd_nosuchdomain000/domain-units?parentDomainUnitIdentifier={child}"),
        ),
    ] {
        let answer = dz.call("GET", &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str(), answer.s("message")),
            (
                403,
                "AccessDeniedException",
                "User is not permitted to perform operation: ListDomainUnitsForParent".to_string()
            ),
            "case {label:?}: expected the modelled denial, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // Inside a domain the caller may list, each row is a request that must be refused,
    // and why: this operation models no ResourceNotFoundException, so an identifier that
    // names nothing in that domain is bad input.
    let cases = vec![
        (
            "a parent unit that is not stored in this domain",
            format!("/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier=nosuchparent"),
        ),
        (
            "a nextToken this service never issued",
            format!(
                "/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={root}&nextToken=notatoken"
            ),
        ),
        // One past each published bound of the modelled `maxResults` range (1..25).
        (
            "maxResults below the modelled range",
            format!(
                "/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={root}&maxResults=0"
            ),
        ),
        (
            "maxResults above the modelled range",
            format!(
                "/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={root}&maxResults=26"
            ),
        ),
    ];

    for (label, uri) in cases {
        let answer = dz.call("GET", &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // The refusals left the hierarchy exactly as it was.
    let listed = reported(&list(&dz, &domain, &root, "").await);
    assert_eq!(
        listed,
        vec![(child, "a child".to_string())],
        "the refused calls must not have changed the stored hierarchy, got {listed:?}"
    );
}
