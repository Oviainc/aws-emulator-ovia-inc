// SPDX-License-Identifier: CC0-1.0

//! `ListCertificateAuthorities`, driven over the wire through the same router the
//! emulator boots.
//!
//! The rules under test, none of them about one particular request:
//!
//! * the listing reports every CA the account owns, each exactly once, in ONE stable
//!   total order, however the walk is cut into pages;
//! * a page carries at most what the caller asked for and never more than the hundred
//!   items the action returns ("Although the maximum value is 1000, the action only
//!   returns a maximum of 100 items.", `ListCertificateAuthoritiesRequest$MaxResults`);
//! * a `NextToken` this listing did not issue is REFUSED with
//!   `InvalidNextTokenException`, never read as "start again from the beginning";
//! * `ResourceOwner` selects by the CA's owner account, and defaults to `SELF`;
//! * listing is reporting: what it reports is what `DescribeCertificateAuthority`
//!   reports, and listing twice reports the same thing.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`]; every test here builds its own, so one
//! test's CAs are invisible to another.

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    encode_page_token, LISTING_CERTIFICATE_AUTHORITIES, MAX_LIST_PAGE_SIZE,
    STATUS_PENDING_CERTIFICATE,
};

use crate::private_ca_lifecycle::{
    root_ca_request, string, wait_for_the_clock, Emulator, ACCOUNT,
};

/// Create `count` private CAs and return their ARNs in the order the listing must
/// report them (ascending by ARN).
async fn created_authorities(emulator: &Emulator, count: usize) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
    for index in 0..count {
        let case = format!("ca-{index}");
        let created = emulator
            .ok(&case, "CreateCertificateAuthority", &root_ca_request(&case))
            .await;
        arns.push(string(&created, "CertificateAuthorityArn", &case));
    }
    arns.sort();
    arns
}

/// A `ListCertificateAuthorities` request carrying only the members given.
fn list_request(
    max_results: Option<i64>,
    next_token: Option<&str>,
    resource_owner: Option<&str>,
) -> Value {
    let mut body = serde_json::Map::new();
    if let Some(size) = max_results {
        body.insert("MaxResults".to_string(), json!(size));
    }
    if let Some(token) = next_token {
        body.insert("NextToken".to_string(), json!(token));
    }
    if let Some(owner) = resource_owner {
        body.insert("ResourceOwner".to_string(), json!(owner));
    }
    Value::Object(body)
}

/// Most items a page asked for by `max_results` may carry.
fn page_cap(max_results: Option<i64>) -> usize {
    let ceiling = i64::from(MAX_LIST_PAGE_SIZE);
    max_results.unwrap_or(ceiling).min(ceiling) as usize
}

/// One full walk of the listing at `max_results` per page: every reported CA, in order,
/// with the number of pages it took.
async fn walk(
    emulator: &Emulator,
    case: &str,
    max_results: Option<i64>,
    resource_owner: Option<&str>,
) -> (Vec<Value>, usize) {
    let cap = page_cap(max_results);
    let mut reported: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let request = list_request(max_results, next_token.as_deref(), resource_owner);
        let page = emulator
            .ok(case, "ListCertificateAuthorities", &request)
            .await;
        pages += 1;
        let authorities = page
            .get("CertificateAuthorities")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a CertificateAuthorities list, got {page:?}")
            })
            .clone();
        assert!(
            authorities.len() <= cap,
            "case {case:?}: expected at most {cap} CAs on a page, got {}",
            authorities.len()
        );
        reported.extend(authorities);
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &next_token {
            None => return (reported, pages),
            Some(token) => assert!(
                pages <= 256,
                "case {case:?}: pagination did not terminate after {pages} pages (token {token:?})"
            ),
        }
    }
}

/// The `Arn` of every reported CA.
fn arns_of(reported: &[Value], case: &str) -> Vec<String> {
    reported
        .iter()
        .map(|authority| string(authority, "Arn", case))
        .collect()
}

#[tokio::test]
async fn every_created_certificate_authority_is_reported_exactly_once_at_every_page_size() {
    let emulator = Emulator::new();
    let created = created_authorities(&emulator, 7).await;

    // The rows differ along the page size: one item per page, a few, exactly the set,
    // one more than the set, the ceiling the action publishes, the maximum the model's
    // `@range(1..1000)` admits, and no `MaxResults` at all.
    for max_results in [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(7),
        Some(8),
        Some(i64::from(MAX_LIST_PAGE_SIZE)),
        Some(1000),
    ] {
        let case = format!("MaxResults {max_results:?}");
        let (reported, pages) = walk(&emulator, &case, max_results, None).await;
        let arns = arns_of(&reported, &case);
        assert_eq!(
            arns, created,
            "case {case:?}: expected every created CA exactly once in one stable order, got {arns:?}"
        );
        let cap = page_cap(max_results);
        let expected_pages = created.len().div_ceil(cap);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of at most {cap}, took {pages}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_at_most_the_hundred_items_the_action_returns() {
    let emulator = Emulator::new();
    // One more CA than the published ceiling, so the ceiling is observable.
    let created = created_authorities(&emulator, usize::try_from(MAX_LIST_PAGE_SIZE).unwrap() + 1)
        .await;
    let ceiling = usize::try_from(MAX_LIST_PAGE_SIZE).unwrap();

    // Each row asks for at least the whole set: an absent MaxResults, the ceiling
    // itself, one past it, and the model's maximum.
    for max_results in [
        None,
        Some(i64::from(MAX_LIST_PAGE_SIZE)),
        Some(i64::from(MAX_LIST_PAGE_SIZE) + 1),
        Some(1000),
    ] {
        let case = format!("MaxResults {max_results:?} over {} CAs", created.len());
        let first = emulator
            .ok(
                &case,
                "ListCertificateAuthorities",
                &list_request(max_results, None, None),
            )
            .await;
        let page = first
            .get("CertificateAuthorities")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {first:?}"));
        assert_eq!(
            page.len(),
            ceiling,
            "case {case:?}: expected the page to stop at {ceiling} CAs, got {}",
            page.len()
        );
        assert!(
            first.get("NextToken").and_then(Value::as_str).is_some(),
            "case {case:?}: expected a NextToken for the CAs left over, got {first:?}"
        );

        let (reported, _) = walk(&emulator, &case, max_results, None).await;
        assert_eq!(
            arns_of(&reported, &case),
            created,
            "case {case:?}: expected the walk to reach every one of the {} CAs",
            created.len()
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused_rather_than_restarting_it() {
    let emulator = Emulator::new();
    let created = created_authorities(&emulator, 3).await;

    // Rows differ along HOW the token fails to be one of this listing's cursors.
    let refused = [
        (
            "a token that is not this service's encoding",
            "not-a-pagination-token".to_string(),
        ),
        ("a token of odd hex length", "abc".to_string()),
        (
            "hex that decodes to no cursor at all",
            "deadbeef".to_string(),
        ),
        (
            "text that carries no listing stamp",
            hex::encode(LISTING_CERTIFICATE_AUTHORITIES),
        ),
        (
            "a cursor stamped with another listing",
            encode_page_token("permissions", &created[0]),
        ),
        (
            "a cursor whose key is empty",
            encode_page_token(LISTING_CERTIFICATE_AUTHORITIES, ""),
        ),
    ];
    for (case, token) in refused {
        let body = emulator
            .refused(
                case,
                "ListCertificateAuthorities",
                &list_request(None, Some(&token), None),
                "InvalidNextTokenException",
            )
            .await;
        assert!(
            body.get("CertificateAuthorities").is_none(),
            "case {case:?}: a refusal must report no page, got {body:?}"
        );
    }

    // A cursor is a CUT into the order, not the name of a CA: one this listing issued
    // resumes exactly where the previous page stopped, and one naming a key past every
    // CA is a well-formed cursor onto the empty tail.
    let first = emulator
        .ok(
            "the first page of one",
            "ListCertificateAuthorities",
            &list_request(Some(1), None, None),
        )
        .await;
    let issued = string(&first, "NextToken", "the first page of one");
    let case = "resuming on the issued cursor";
    let page = emulator
        .ok(
            case,
            "ListCertificateAuthorities",
            &list_request(None, Some(&issued), None),
        )
        .await;
    let resumed = arns_of(
        page.get("CertificateAuthorities")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {page:?}")),
        case,
    );
    assert_eq!(
        resumed,
        created[1..].to_vec(),
        "case {case:?}: expected the CAs after the first page, got {resumed:?}"
    );

    let case = "a cursor onto the empty tail";
    let beyond = encode_page_token(
        LISTING_CERTIFICATE_AUTHORITIES,
        &format!("{}~", created[created.len() - 1]),
    );
    let tail = emulator
        .ok(
            case,
            "ListCertificateAuthorities",
            &list_request(None, Some(&beyond), None),
        )
        .await;
    assert_eq!(
        tail.get("CertificateAuthorities")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "case {case:?}: expected an empty page, got {tail:?}"
    );
    assert!(
        tail.get("NextToken").and_then(Value::as_str).is_none(),
        "case {case:?}: expected no NextToken past the last CA, got {tail:?}"
    );
}

#[tokio::test]
async fn max_results_is_accepted_only_inside_the_range_the_model_publishes() {
    let emulator = Emulator::new();
    created_authorities(&emulator, 2).await;

    // `MaxResults` is `@range(min: 1, max: 1000)`
    // (smithy_query(query="operation", operation="ListCertificateAuthorities")): each
    // published bound is accepted and the value just past it is refused.
    for max_results in [1, 2, 500, 1000] {
        let case = format!("MaxResults {max_results} inside the range");
        let (status, body) = emulator
            .call(
                "ListCertificateAuthorities",
                &list_request(Some(max_results), None, None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );
    }
    for max_results in [0, -1, 1001, 100_000] {
        let case = format!("MaxResults {max_results} outside the range");
        let (status, body) = emulator
            .call(
                "ListCertificateAuthorities",
                &list_request(Some(max_results), None, None),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the constraint to refuse the request, got {status} with {body:?}"
        );
    }

    // `NextToken` is `@length(min: 1, max: 43739)`: a token below the lower bound is
    // refused rather than read as "no cursor".
    let (status, body) = emulator
        .call("ListCertificateAuthorities", &list_request(None, Some(""), None))
        .await;
    assert_eq!(
        status, 400,
        "case \"an empty NextToken\": expected a refusal, got {status} with {body:?}"
    );
}

#[tokio::test]
async fn resource_owner_selects_the_certificate_authorities_that_account_owns() {
    let emulator = Emulator::new();
    let created = created_authorities(&emulator, 4).await;

    // `ResourceOwner` is "OTHER_ACCOUNTS | SELF" and "The default is SELF."; every CA
    // this emulator mints is owned by the account it presents, so SELF is all of them
    // and OTHER_ACCOUNTS is none of them.
    let cases: [(&str, Option<&str>, Vec<String>); 3] = [
        ("the default owner", None, created.clone()),
        ("SELF", Some("SELF"), created.clone()),
        ("OTHER_ACCOUNTS", Some("OTHER_ACCOUNTS"), Vec::new()),
    ];
    for (case, resource_owner, expected) in cases {
        let (reported, pages) = walk(&emulator, case, None, resource_owner).await;
        assert_eq!(
            arns_of(&reported, case),
            expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(pages, 1, "case {case:?}: expected one page, took {pages}");
        for authority in &reported {
            assert_eq!(
                string(authority, "OwnerAccount", case),
                ACCOUNT,
                "case {case:?}: expected every reported CA to be owned by {ACCOUNT}, got {authority:?}"
            );
        }
    }

    // A value outside the modelled enum is refused, not read as "not SELF": an empty
    // 200 would let a request the real service rejects look like an answer.
    for owner in ["self", "SELF ", "OTHER_ACCOUNT", "ALL"] {
        let case = format!("ResourceOwner {owner:?}");
        let (status, body) = emulator
            .call(
                "ListCertificateAuthorities",
                &list_request(None, None, Some(owner)),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a value outside ResourceOwner to be refused, got {status} with {body:?}"
        );
    }
}

#[tokio::test]
async fn a_listed_certificate_authority_is_what_describe_reports_and_listing_changes_nothing() {
    let emulator = Emulator::new();
    let created = created_authorities(&emulator, 3).await;
    wait_for_the_clock();

    let case = "the settled listing";
    let (reported, _) = walk(&emulator, case, None, None).await;
    assert_eq!(
        arns_of(&reported, case),
        created,
        "case {case:?}: expected every created CA, got {reported:?}"
    );

    for authority in &reported {
        let arn = string(authority, "Arn", case);
        assert_eq!(
            string(authority, "Status", case),
            STATUS_PENDING_CERTIFICATE,
            "case {arn:?}: a CA whose creation has settled waits for its certificate, got {authority:?}"
        );
        let described = emulator
            .ok(
                &arn,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        assert_eq!(
            described.get("CertificateAuthority"),
            Some(authority),
            "case {arn:?}: expected the listed CA to be the described CA, got {described:?}"
        );
    }

    // Listing is reporting: a second walk, and a walk cut into single-item pages, report
    // exactly what the first one did.
    let (again, _) = walk(&emulator, "the listing read twice", None, None).await;
    assert_eq!(
        again, reported,
        "case \"the listing read twice\": a read must not change what a later caller sees"
    );
    let (by_ones, _) = walk(&emulator, "the listing one CA at a time", Some(1), None).await;
    assert_eq!(
        by_ones, reported,
        "case \"the listing one CA at a time\": paging must not change what is reported"
    );
}
