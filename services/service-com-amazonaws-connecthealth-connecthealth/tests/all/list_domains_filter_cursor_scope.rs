// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage that a `ListDomains` cursor is a resume point only for the
//! collection it was issued for — EVERY value the request narrows that collection by.
//!
//! The RULE under test, asserted over a table rather than the one cursor the issue
//! recorded: `ListDomains` is
//! `@paginated(inputToken=nextToken, outputToken=nextToken, pageSize=maxResults)` over an
//! inventory narrowed by the Region the request is signed for AND by its `status` filter
//! (`smithy_query(query="operation",operation="ListDomains",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! reports `status` as an `httpQuery` member targeting `DomainStatus`, documented as
//! "Filter by Domain status"), and its cursor is an opaque "resume strictly after this
//! key" position. So a cursor is a resume point ONLY for a request that narrows the
//! inventory the same way: a cursor issued under one narrowing is refused by a listing
//! narrowed differently, instead of skipping every record sorting at or before its key —
//! records the resuming listing has never served.
//!
//! The rows differ along what the rule generalises over: which narrowing value the cursor
//! was issued under (each `DomainStatus` of the model's catalog and the unfiltered
//! listing, crossed with this Region and another), which narrowing the resuming request
//! states (the same four), and which key the cursor carries (the lowest and the highest
//! id the store holds, so acceptance cannot depend on the resume point's position). The
//! accepting half is asserted too — a cursor whose narrowing MATCHES resumes the listing
//! and serves only records of that narrowing — so "refuse a cursor of another collection"
//! cannot be satisfied by refusing every cursor.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) and the `webAppSetupConfiguration`
//! that makes a deletion outlive its call are the sibling suites': one way of driving this
//! service, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    domains_page_scope, encode_page_token, DomainStatus,
};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The Region every request of a case is signed for, and a second Region whose listings
/// are a DIFFERENT collection of the same kind.
const REGION: &str = "eu-west-1";
const OTHER_REGION: &str = "ap-south-1";

/// The narrowings a `ListDomains` request can state: no filter at all, and each value of
/// the model's `DomainStatus` catalog
/// (`smithy_query(query="shape",shape="DomainStatus",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
/// — `ACTIVE | DELETED | DELETING`). `DELETED` is in the table although no record ever
/// reports it: a cursor of that collection is still a cursor of a collection, and the rule
/// is about the narrowing, not about how many records it selects.
const FILTERS: [Option<&str>; 4] = [None, Some("ACTIVE"), Some("DELETING"), Some("DELETED")];

/// The body member a refused ConnectHealth call reports its detail text in.
const DETAIL_MEMBER: &str = "Message";

/// This service's refusal of a `nextToken` it did not issue for the collection being
/// listed, as its one cursor refusal words it.
const UNISSUED_DETAIL: &str = "nextToken is not a token this service issued";

/// The status a filter names, panicking on a value outside the catalog above so a typo in
/// the table cannot pass for "no filter".
fn status_of(filter: Option<&str>) -> Option<DomainStatus> {
    filter.map(|value| {
        DomainStatus::from_wire(value)
            .unwrap_or_else(|| panic!("{value} must be a DomainStatus of the model's catalog"))
    })
}

/// `/domain` with the narrowing `filter` states, and optionally a cursor and a page size.
fn listing_path(filter: Option<&str>, token: Option<&str>, max_results: Option<usize>) -> String {
    let mut path = "/domain".to_string();
    for (name, value) in [
        ("status", filter.map(ToString::to_string)),
        ("nextToken", token.map(ToString::to_string)),
        ("maxResults", max_results.map(|size| size.to_string())),
    ] {
        if let Some(value) = value {
            let separator = if path.contains('?') { '&' } else { '?' };
            path = format!("{path}{separator}{name}={value}");
        }
    }
    path
}

/// The `(domainId, status)` pairs of one page, in the order the listing reported them.
fn entries(case: &str, body: &Value) -> Vec<(String, String)> {
    body.get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: ListDomains must carry a domains list, got {body}"))
        .iter()
        .map(|entry| (string_at(entry, "domainId"), string_at(entry, "status")))
        .collect()
}

/// The detail text of a refusal, failing with the whole body when it carries none.
fn detail_text(case: &str, body: &Value) -> String {
    body.get(DETAIL_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a refusal must carry a {DETAIL_MEMBER} member, got {body}")
        })
        .to_string()
}

#[tokio::test]
async fn a_domains_cursor_resumes_only_under_the_narrowing_it_was_issued_under() {
    let emulator = Emulator::new(REGION);
    let mut held: BTreeSet<String> = BTreeSet::new();
    for index in 0..4 {
        let created = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        held.insert(string_at(&created, "domainId"));
    }
    let lowest = held.iter().next().expect("a created domain").clone();
    let highest = held.iter().next_back().expect("a created domain").clone();

    // Every collection a cursor of this listing can be issued for: the two Regions crossed
    // with the four narrowings, each at the lowest and at the highest key the store holds.
    for issuing_region in [REGION, OTHER_REGION] {
        for issuing_filter in FILTERS {
            for key in [&lowest, &highest] {
                let cursor = encode_page_token(
                    &domains_page_scope(issuing_region, status_of(issuing_filter)),
                    key,
                );
                for resuming_filter in FILTERS {
                    let case = format!(
                        "a cursor of {issuing_region}/{issuing_filter:?} after {key} resumed on \
                         {REGION}/{resuming_filter:?}"
                    );
                    let path = listing_path(resuming_filter, Some(&cursor), None);
                    let (status, body) = emulator.send("GET", &path, None).await;
                    // Accepted exactly when the cursor's collection IS the one being
                    // listed: both narrowing values have to agree.
                    let same_collection =
                        issuing_region == REGION && issuing_filter == resuming_filter;
                    if same_collection {
                        assert_eq!(
                            status, 200,
                            "case {case:?}: a cursor of this very collection must resume it, \
                             got {status} with {body}"
                        );
                        for (domain_id, reported) in entries(&case, &body) {
                            assert!(
                                domain_id > *key,
                                "case {case:?}: a resumed page starts strictly after the \
                                 cursor's key, got {domain_id}"
                            );
                            if let Some(wanted) = resuming_filter {
                                assert_eq!(
                                    reported, wanted,
                                    "case {case:?}: a filtered page reports only that status, \
                                     got {domain_id} as {reported}"
                                );
                            }
                        }
                    } else {
                        assert_eq!(
                            status, 400,
                            "case {case:?}: a cursor of another collection must be refused, \
                             got {status} with {body}"
                        );
                        assert_eq!(
                            error_code(&body),
                            "ValidationException",
                            "case {case:?}: got {body}"
                        );
                        let text = detail_text(&case, &body);
                        assert_eq!(
                            text, UNISSUED_DETAIL,
                            "case {case:?}: expected this service's cursor refusal, got {text:?}"
                        );
                    }
                }
            }
        }
    }

    // A refused cursor changes nothing: the unfiltered listing still reports every domain.
    let (status, body) = emulator
        .send("GET", &listing_path(None, None, Some(100)), None)
        .await;
    assert_eq!(status, 200, "ListDomains got {status} with {body}");
    let listed: BTreeSet<String> = entries("the whole listing", &body)
        .into_iter()
        .map(|(domain_id, _)| domain_id)
        .collect();
    assert_eq!(
        listed, held,
        "a refused cursor must leave the listing itself unchanged"
    );
}

#[tokio::test]
async fn paging_one_narrowing_never_drops_a_domain_of_another() {
    let emulator = Emulator::new(REGION);
    // Two domains whose deletion finishes inside the call (so the inventory keeps them
    // ACTIVE) and two owning a web application (so their deletion passes through the
    // non-terminal DELETING), which is what gives the listing two non-empty narrowings.
    let mut active: BTreeSet<String> = BTreeSet::new();
    for index in 0..2 {
        let created = emulator
            .create_domain(&json!({ "name": format!("clinic-{index}") }))
            .await;
        active.insert(string_at(&created, "domainId"));
    }
    let mut deleting: BTreeSet<String> = BTreeSet::new();
    for index in 0..2 {
        let created = emulator
            .create_domain(&json!({
                "name": format!("web-clinic-{index}"),
                "webAppSetupConfiguration": web_app_setup(),
            }))
            .await;
        deleting.insert(string_at(&created, "domainId"));
    }
    for domain_id in &deleting {
        let (status, deleted) = emulator
            .send("DELETE", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "DeleteDomain on {domain_id} got {status} with {deleted}"
        );
        assert_eq!(
            string_at(&deleted, "status"),
            "DELETING",
            "a web-app domain's deletion outlives the call, got {deleted}"
        );
    }
    let everything: BTreeSet<String> = active.union(&deleting).cloned().collect();

    // The cursor the narrowed listing issues, replayed on the UNFILTERED listing: the
    // collections differ, so it is no resume point for it — and the unfiltered listing
    // still serves every domain it has always served.
    let (status, page) = emulator
        .send("GET", &listing_path(Some("DELETING"), None, Some(1)), None)
        .await;
    assert_eq!(status, 200, "ListDomains got {status} with {page}");
    let issued = page
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a page of one of two DELETING domains carries a cursor: {page}"))
        .to_string();

    let (status, body) = emulator
        .send("GET", &listing_path(None, Some(&issued), None), None)
        .await;
    assert_eq!(
        status, 400,
        "a cursor of the DELETING collection must not resume the unfiltered listing, \
         got {status} with {body}"
    );
    assert_eq!(error_code(&body), "ValidationException", "got {body}");

    // Walking each narrowing with only the cursors IT issued covers that narrowing's
    // records exactly once, and the narrowings together cover the whole inventory — the
    // property a cursor accepted across narrowings breaks.
    for (filter, expected) in [
        (None, everything.clone()),
        (Some("ACTIVE"), active.clone()),
        (Some("DELETING"), deleting.clone()),
        (Some("DELETED"), BTreeSet::new()),
    ] {
        for page_size in 1..=3 {
            let case = format!("{filter:?} paged at {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let path = listing_path(filter, token.as_deref(), Some(page_size));
                let (status, body) = emulator.send("GET", &path, None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: page {pages} got {status} with {body}"
                );
                for (domain_id, reported) in entries(&case, &body) {
                    if let Some(wanted) = filter {
                        assert_eq!(
                            reported, wanted,
                            "case {case:?}: expected only {wanted} domains, got {domain_id} as \
                             {reported}"
                        );
                    }
                    seen.push(domain_id);
                }
                pages += 1;
                assert!(
                    pages <= 6,
                    "case {case:?}: paging did not finish, saw {seen:?}"
                );
                token = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if token.is_none() {
                    break;
                }
            }
            let distinct: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                distinct.len(),
                seen.len(),
                "case {case:?}: a domain was reported twice across pages, saw {seen:?}"
            );
            assert_eq!(
                distinct, expected,
                "case {case:?}: paging must cover exactly the matching domains, saw {seen:?}"
            );
        }
    }
}
