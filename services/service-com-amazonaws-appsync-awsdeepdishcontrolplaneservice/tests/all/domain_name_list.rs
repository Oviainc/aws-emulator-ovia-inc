// SPDX-License-Identifier: CC0-1.0

//! `ListDomainNames`, end to end over HTTP: what a page holds, what resumes it, and
//! which listings are refused.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: a list call reports every custom domain name this
//! account holds, in one order that does not depend on how the caller pages, cut into a
//! page of at most `maxResults` entries (the modelled maximum when the request names
//! none) and resumed by the `nextToken` the previous page handed back, which is reported
//! exactly while entries remain; each entry is the `DomainNameConfig` the create stored
//! and `GetDomainName` answers for that domain; and the call is a pure read, so paging
//! leaves every later reader seeing the same set.
//!
//! The rows span:
//!
//! 1. the count of domains the account holds — none, one, several, and the whole
//!    service quota, which is exactly one modelled page — so neither the page nor its
//!    cursor can be keyed to a single collection size;
//! 2. the `maxResults` the caller names — unspecified, the modelled default sent
//!    explicitly, and values inside the modelled range up to its published maximum;
//! 3. the FORM of the listed domain names, rotated across every form the modelled
//!    pattern admits (two labels, three labels, digits and hyphens, both wildcard
//!    forms, the modelled maximum length), and the form of the certificate, the
//!    presence of `description` and the presence of `tags`, so the projection cannot be
//!    keyed to one kind of domain;
//! 4. the forms of a resume that names no page of the list.
//!
//! The harness (`call`, `router`, `Answer`) is the source-API association suite's —
//! reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, router, Answer};

/// `ListDomainNames`' and `CreateDomainName`'s shared path. Source: the `http` binding
/// of `ListDomainNames`, `smithy_query(query="operation", operation="ListDomainNames")`.
const DOMAIN_NAMES_PATH: &str = "/v1/domainnames";

/// The largest `domainName` the model admits: `DomainName` is `@length` 1..253.
/// Source: `smithy_query(query="operation", operation="CreateDomainName")`.
const DOMAIN_NAME_MAX_LEN: usize = 253;

/// The AppSync service quota for custom domain names an account holds in one Region,
/// which bounds the store a listing pages through;
/// `ops::create_domain_name::DOMAIN_NAMES_PER_REGION` is the service's own copy.
const DOMAIN_NAMES_PER_REGION: usize = 25;

/// The counts of domains the account is listed at: none, one, several, and the whole
/// service quota — which is also exactly one modelled page.
const COUNTS: &[usize] = &[0, 1, 4, DOMAIN_NAMES_PER_REGION];

/// The `maxResults` values a walk pages with: unspecified, the modelled default sent
/// explicitly, and values inside the modelled range up to its published maximum.
/// Source: the `maxResults` member of `ListDomainNamesRequest` (`@range` 0..25,
/// default 0 — i.e. unspecified, so `0` and an absent member select the same page).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(7), Some(25)];

/// The two forms `CertificateArn`'s pattern admits: an ACM certificate and an IAM
/// server certificate. Source: the `certificateArn` member of
/// `CreateDomainNameRequest`.
const CERTIFICATE_PREFIXES: &[&str] = &[
    "arn:aws:acm:us-east-1:000000000000:certificate/listed-",
    "arn:aws:iam::000000000000:server-certificate/listed_",
];

/// A domain name of the modelled maximum length that still carries `index`, so every
/// seeded domain is a different resource. Its first label absorbs the padding, which
/// the modelled pattern admits.
fn max_length_domain(index: usize) -> String {
    let tail = format!("d{index:04}.example.com");
    let label_len = DOMAIN_NAME_MAX_LEN - tail.len() - 1;
    format!("{}.{tail}", "a".repeat(label_len))
}

/// The domain name the domain at `index` of a seeded account carries. The FORM rotates
/// across every shape the modelled `DomainName` pattern
/// (`^(\*[\w\d-]*\.)?([\w\d-]+\.)+[\w\d-]+$`, `@length` 1..253) admits, and `index`
/// keeps each one distinct.
fn seeded_domain(index: usize) -> String {
    match index % 6 {
        0 => format!("d{index:04}.example"),
        1 => format!("api.d{index:04}.example.com"),
        2 => format!("api-7.d{index:04}-x.example.com"),
        3 => format!("*.d{index:04}.example.com"),
        4 => format!("*dev.d{index:04}.example.com"),
        _ => max_length_domain(index),
    }
}

/// `CreateDomainName` for the domain at `index`: its certificate form, and whether it
/// names a `description` and `tags`, rotate with the index too.
async fn create_seeded_domain(router: &MultiServiceRouter, index: usize) -> Value {
    let domain = seeded_domain(index);
    let certificate_arn = format!(
        "{}{index:04}",
        CERTIFICATE_PREFIXES[index % CERTIFICATE_PREFIXES.len()]
    );
    let mut body = json!({
        "domainName": domain,
        "certificateArn": certificate_arn,
    });
    match index % 3 {
        0 => {}
        1 => body["description"] = json!(""),
        _ => body["description"] = json!(format!("the domain at {index}")),
    }
    if index.is_multiple_of(2) {
        body["tags"] = json!({"stage": "beta", "index": index.to_string()});
    }
    let answer = call(router, "POST", DOMAIN_NAMES_PATH, Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case seed {domain:?}: expected CreateDomainName 200, got {answer:?}"
    );
    answer.body["domainNameConfig"].clone()
}

/// Create `count` custom domains and return their names in creation order.
async fn seed(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut domains = Vec::with_capacity(count);
    for index in 0..count {
        create_seeded_domain(router, index).await;
        domains.push(seeded_domain(index));
    }
    domains
}

/// One page of a listing: the domain names it reported, the entries themselves, and the
/// cursor that resumes after them.
#[derive(Debug)]
struct ListedPage {
    names: Vec<String>,
    configs: Vec<Value>,
    next_token: Option<String>,
}

/// Send one `ListDomainNames`, with the `maxResults` and `nextToken` the caller names.
async fn list_page(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let path = if query.is_empty() {
        DOMAIN_NAMES_PATH.to_string()
    } else {
        format!("{DOMAIN_NAMES_PATH}?{}", query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// One page as the suite reads it, asserting it is a 200 that reports the list member.
fn read_page(case: &str, answer: &Answer) -> ListedPage {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListDomainNames 200, got {answer:?}"
    );
    let configs: Vec<Value> = answer.body["domainNameConfigs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a domainNameConfigs list, got {answer:?}"))
        .clone();
    let names = configs
        .iter()
        .map(|config| {
            config["domainName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected every entry to carry a domainName, got {answer:?}")
                })
                .to_string()
        })
        .collect();
    ListedPage {
        names,
        configs,
        next_token: answer.body["nextToken"].as_str().map(str::to_string),
    }
}

/// Page through the whole account at `max_results` and return every entry, in the order
/// the listing reported them, asserting the page-size bound and the cursor contract on
/// each page.
async fn walk(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    expected_total: usize,
) -> Vec<Value> {
    // An unspecified `maxResults` — absent, or the modelled default of `0` — selects
    // the whole modelled page.
    let bound = match max_results {
        Some(size) if size > 0 => size as usize,
        _ => state::DEFAULT_PAGE_SIZE,
    };
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A page carries at least one entry while entries remain, so the walk cannot take
    // more pages than there are entries, plus the single page of an empty listing.
    for page_index in 0..expected_total + 1 {
        let case = format!("walk at maxResults {max_results:?}, page {page_index} from {token:?}");
        let answer = list_page(router, max_results, token.as_deref()).await;
        let page = read_page(&case, &answer);
        assert!(
            page.names.len() <= bound,
            "case {case}: expected at most {bound} entries, got {} ({:?})",
            page.names.len(),
            page.names
        );
        let remaining = expected_total - collected.len();
        assert_eq!(
            page.names.len(),
            remaining.min(bound),
            "case {case}: expected the page to be as full as the remainder allows, got {:?}",
            page.names
        );
        collected.extend(page.configs);
        // The cursor is reported exactly while entries remain after this page.
        let more = collected.len() < expected_total;
        assert_eq!(
            page.next_token.is_some(),
            more,
            "case {case}: expected a nextToken {}, got {:?}",
            if more { "while entries remain" } else { "on the last page" },
            page.next_token
        );
        match page.next_token {
            None => return collected,
            Some(cursor) => token = Some(cursor),
        }
    }
    panic!("case walk at maxResults {max_results:?}: the walk never reached a last page");
}

/// The domain names of a walk's entries, in the order the listing reported them.
fn listed_names(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|config| config["domainName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `GetDomainName` for `domain_name`.
async fn get_domain(router: &MultiServiceRouter, domain_name: &str) -> Answer {
    call(router, "GET", &format!("{DOMAIN_NAMES_PATH}/{domain_name}"), None).await
}

/// Every walk of the account reports the same domains in the same order, however the
/// caller pages, and every domain it holds exactly once.
#[tokio::test]
async fn every_page_size_walks_the_whole_account_in_one_order() {
    for count in COUNTS.iter().copied() {
        // A fresh store per row: the service quota bounds how many domains one account
        // holds, so each count is seeded on its own engine.
        let router = router();
        let mut seeded = seed(&router, count).await;
        seeded.sort();

        let mut reference: Option<Vec<String>> = None;
        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(&router, max_results, count).await;
            let names = listed_names(&listed);
            // Every seeded domain, exactly once: a set comparison that is blind to the
            // order, which the next assertion pins separately.
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                sorted, seeded,
                "case {count} domains at maxResults {max_results:?}: expected every seeded \
                 domain exactly once, got {names:?}"
            );
            match &reference {
                None => reference = Some(names),
                Some(reference) => assert_eq!(
                    &names, reference,
                    "case {count} domains at maxResults {max_results:?}: expected the same order \
                     as the first walk {reference:?}, got {names:?}"
                ),
            }
        }
    }
}

/// Each listed entry is the `DomainNameConfig` the create answered and `GetDomainName`
/// answers for that domain, member for member — so a listing projects the stored domain
/// rather than a summary of it.
#[tokio::test]
async fn a_listed_entry_is_the_config_create_and_get_answer() {
    let router = router();
    // One domain per form the modelled pattern admits, so the projection is asserted on
    // every kind of domain, certificate, description and tag map the table spans.
    let count = 6;
    let mut created: Vec<(String, Value)> = Vec::with_capacity(count);
    for index in 0..count {
        let config = create_seeded_domain(&router, index).await;
        created.push((seeded_domain(index), config));
    }

    for max_results in PAGE_SIZES.iter().copied() {
        let listed = walk(&router, max_results, count).await;
        for entry in &listed {
            let name = entry["domainName"].as_str().unwrap_or_default();
            let read = get_domain(&router, name).await;
            assert_eq!(
                read.status, 200,
                "case listed {name:?} at maxResults {max_results:?}: expected GetDomainName 200, \
                 got {read:?}"
            );
            assert_eq!(
                &read.body["domainNameConfig"], entry,
                "case listed {name:?} at maxResults {max_results:?}: expected the listing to \
                 answer the stored record {:?}, got {entry:?}",
                read.body["domainNameConfig"]
            );
            let (_, at_create) = created
                .iter()
                .find(|(domain, _)| domain == name)
                .unwrap_or_else(|| panic!("case listed {name:?}: expected a seeded domain"));
            assert_eq!(
                at_create, entry,
                "case listed {name:?} at maxResults {max_results:?}: expected the config the \
                 create answered {at_create:?}, got {entry:?}"
            );
        }
    }
}

/// Listing is a pure read: paging through the account changes nothing a later caller
/// sees, so the walk repeats identically and every domain still reads back.
#[tokio::test]
async fn paging_leaves_the_stored_set_unchanged() {
    let router = router();
    let count = 9;
    let seeded = seed(&router, count).await;

    // A first walk at the smallest page, so the listing is paged all the way through...
    let first = walk(&router, Some(1), count).await;
    // ... then the same walk again, and one at the modelled maximum.
    let again = walk(&router, Some(1), count).await;
    let whole = walk(&router, Some(25), count).await;

    assert_eq!(
        again, first,
        "case a repeated walk: expected the same entries as the first walk {first:?}, got \
         {again:?}"
    );
    assert_eq!(
        whole, first,
        "case a walk at the modelled maximum after paging: expected the same entries {first:?}, \
         got {whole:?}"
    );
    for domain in &seeded {
        let read = get_domain(&router, domain).await;
        assert_eq!(
            read.status, 200,
            "case {domain:?} after paging: expected GetDomainName 200, got {read:?}"
        );
    }
}

/// A domain created after a listing appears in the next one, so the page is cut from
/// the store rather than from anything fixed at boot.
#[tokio::test]
async fn a_listing_reports_the_domains_the_store_holds_now() {
    let router = router();
    let total = 5;
    for grown_to in 0..=total {
        let listed = walk(&router, Some(2), grown_to).await;
        let mut names = listed_names(&listed);
        names.sort();
        let mut expected: Vec<String> = (0..grown_to).map(seeded_domain).collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case after {grown_to} creates: expected exactly the stored domains {expected:?}, \
             got {names:?}"
        );
        if grown_to < total {
            create_seeded_domain(&router, grown_to).await;
        }
    }
}

/// A resume that names no page of the list is refused with the modelled client error
/// and AppSync's fixed invalid-token text, while a token the listing itself handed back
/// still resumes.
#[tokio::test]
async fn a_resume_that_names_no_page_is_refused() {
    let router = router();
    let count = 4;
    seed(&router, count).await;

    // Tokens that name no page of this list: a domain this account does not hold, a
    // value of the right shape for another resource family, and values of other shapes
    // entirely.
    for token in [
        "bogus",
        "0",
        "absent.example.com",
        "da2-nosuchkey",
        &seeded_domain(count + 1),
    ] {
        let answer = list_page(&router, None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case resuming from {token:?}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case resuming from {token:?}: expected BadRequestException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str().unwrap_or_default(),
            "Invalid pagination token given.",
            "case resuming from {token:?}: expected AppSync's fixed invalid-token text, got \
             {answer:?}"
        );
    }

    // A token the listing itself handed back still resumes, so the refusals above are
    // about tokens that name no page rather than about paging at all.
    let first = list_page(&router, Some(1), None).await;
    let page = read_page("the first page of the refusal account", &first);
    let cursor = page
        .next_token
        .expect("the first page of four domains carries a nextToken");
    let resumed = list_page(&router, Some(1), Some(&cursor)).await;
    let resumed = read_page("resuming from the listing's own token", &resumed);
    assert_eq!(
        resumed.names.len(),
        1,
        "case resuming from the listing's own token: expected one entry, got {:?}",
        resumed.names
    );
    assert_eq!(
        resumed.names,
        vec![cursor.clone()],
        "case resuming from the listing's own token: expected the entry the cursor names \
         ({cursor:?}), got {:?}",
        resumed.names
    );
}
