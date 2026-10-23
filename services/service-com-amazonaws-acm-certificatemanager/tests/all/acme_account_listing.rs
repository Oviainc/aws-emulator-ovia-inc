// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAcmeAccounts` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.ListAcmeAccounts`, awsJson1_1).
//!
//! RULE: `ListAcmeAccounts` REPORTS the ACME accounts the store holds for the endpoint
//! `AcmeEndpointArn` names — every member projected from the stored record, in one
//! deterministic order, paged by `MaxResults` with a `NextToken` that resumes exactly
//! where the previous page stopped — answers `ResourceNotFoundException` for an ARN no
//! stored endpoint answers to, and registers, changes or consumes nothing.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, so a response that is not computed from the
//! persisted store cannot pass. The endpoints are created with `CreateAcmeEndpoint` and
//! read back through `ListAcmeAccounts`, a different operation.
//!
//! The ACCOUNTS, though, are seeded into the store directly, because nothing in this
//! service's API creates one: "ACME accounts are created when clients use external
//! account binding credentials to register" (`ListAcmeAccounts` doc), i.e. by an ACME
//! client's own `newAccount` request to the endpoint's directory URL (RFC 8555 §7.3),
//! which no AWS operation stands in for. Seeding the records is therefore the only way
//! to exercise the projection, the ordering and the paging the rule is about; the
//! records are written with the same `state::AcmeAccountRecord` the handler reads.
//!
//! The table below spans the classes the rule generalises over: every
//! `AcmeAccountStatus` the model declares, an account with and without an external
//! account binding, contact lists of several sizes and scripts, creation instants in an
//! order that disagrees with the account-URL order, and page sizes from 1 through one
//! past the number of accounts as well as both published `MaxResults` bounds and one
//! past each.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{call, error_code, error_message, router_with_engine, REGION};

/// Every `AcmeAccountStatus` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeAccountStatus")`).
const ACCOUNT_STATUSES: &[&str] = &[
    state::ACME_ACCOUNT_STATUS_VALID,
    state::ACME_ACCOUNT_STATUS_DEACTIVATED,
    state::ACME_ACCOUNT_STATUS_REVOKED,
];

/// `MaxResults`' modelled range
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ListAcmeAccountsRequest")`
/// reports `range {min: 1, max: 100}`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// One seeded ACME account: what it exercises, the id its account URL ends in, the
/// `AcmeAccountStatus` it carries, whether it registered against an external account
/// binding, its contact URIs, and how long after the endpoint it registered.
struct AccountCase {
    case: &'static str,
    account_id: &'static str,
    status: &'static str,
    with_binding: bool,
    contacts: Vec<&'static str>,
    registered_after: Duration,
}

/// The accounts seeded on the endpoint under test.
///
/// The ids are deliberately NOT in the order the rows are written, so an answer that
/// echoes insertion order rather than the store's order cannot pass, and the creation
/// instants disagree with the id order for the same reason.
fn account_cases() -> Vec<AccountCase> {
    vec![
        AccountCase {
            case: "valid account, external account binding, one contact",
            account_id: "d4b1c0de-0000-4000-8000-000000000003",
            status: ACCOUNT_STATUSES[0],
            with_binding: true,
            contacts: vec!["mailto:ops@example.com"],
            registered_after: Duration::from_secs(1),
        },
        AccountCase {
            case: "deactivated account, no binding, no contacts",
            account_id: "a1000000-0000-4000-8000-000000000001",
            status: ACCOUNT_STATUSES[1],
            with_binding: false,
            contacts: vec![],
            registered_after: Duration::from_secs(90),
        },
        AccountCase {
            case: "revoked account, binding, several contacts in several scripts",
            account_id: "c3000000-0000-4000-8000-000000000002",
            status: ACCOUNT_STATUSES[2],
            with_binding: true,
            contacts: vec![
                "mailto:sécurité@example.com",
                "mailto:運用@example.jp",
                "mailto:admin@example.org",
            ],
            registered_after: Duration::from_secs(5),
        },
        AccountCase {
            case: "valid account, no binding, contact list at several entries",
            account_id: "b2000000-0000-4000-8000-000000000004",
            status: ACCOUNT_STATUSES[0],
            with_binding: false,
            contacts: vec!["mailto:a@example.com", "mailto:b@example.com"],
            registered_after: Duration::from_secs(3600),
        },
    ]
}

/// `CreateAcmeEndpoint`, asserted to succeed, returning the ARN it minted.
async fn create_endpoint(router: &MultiServiceRouter, case: &str) -> String {
    let body = json!({
        "AuthorizationBehavior": "PRE_APPROVED",
        "Contact": "REQUIRED",
        "CertificateAuthority": { "PublicCertificateAuthority": {} },
    });
    let (status, response) = call(router, "CreateAcmeEndpoint", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAcmeEndpoint must succeed, got {status} {response}"
    );
    response["AcmeEndpointArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAcmeEndpoint returned no AcmeEndpointArn"))
        .to_string()
}

/// The endpoint id of an ACME endpoint ARN: the part after `acme-endpoint/`.
fn endpoint_id(arn: &str) -> &str {
    arn.rsplit_once("acme-endpoint/")
        .unwrap_or_else(|| panic!("{arn} is not an ACME endpoint ARN"))
        .1
}

/// Register the table's accounts with `endpoint_arn` in the store, as an ACME client's
/// own `newAccount` request would, and answer the records that were written.
///
/// Writes nothing but the account records themselves: the account URL is rendered by
/// the emulator's own [`state::acme_account_url`], so the test cannot invent a URL form
/// the handler would not report.
fn register_accounts(
    engine: &Arc<engine::InMemoryEngine>,
    endpoint_arn: &str,
    cases: &[AccountCase],
) -> Vec<state::AcmeAccountRecord> {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let created_at = store
        .acme_endpoints
        .get(endpoint_arn)
        .unwrap_or_else(|| panic!("{endpoint_arn} must be a stored ACME endpoint"))
        .created_at;
    let mut written = Vec::new();
    for case in cases {
        let record = state::AcmeAccountRecord {
            acme_endpoint_arn: endpoint_arn.to_string(),
            account_url: state::acme_account_url(
                REGION,
                endpoint_id(endpoint_arn),
                case.account_id,
            ),
            // A JWK thumbprint is base64url text (RFC 7638 §3); what matters here is
            // that each account carries its own, so a response cannot confuse two.
            public_key_thumbprint: format!("thumbprint-{}", case.account_id),
            status: case.status.to_string(),
            acme_external_account_binding_arn: case.with_binding.then(|| {
                format!("{endpoint_arn}/acme-external-account-binding/{}", case.account_id)
            }),
            contacts: case.contacts.iter().map(|c| c.to_string()).collect(),
            created_at: created_at + case.registered_after,
        };
        store.acme_accounts.insert(
            (endpoint_arn.to_string(), record.account_url.clone()),
            record.clone(),
        );
        written.push(record);
    }
    written
}

/// One `ListAcmeAccounts` call, asserted to succeed, returning the response body.
async fn list_page(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "ListAcmeAccounts", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeAccounts({body}) must succeed, got {status} {response}"
    );
    response
}

/// The `AcmeAccounts` member of a response, as a list.
fn accounts_of(case: &str, response: &Value) -> Vec<Value> {
    response["AcmeAccounts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no AcmeAccounts list in {response}"))
        .clone()
}

/// Every account `ListAcmeAccounts` reports for `endpoint_arn`, paging to the end with
/// `max_results` per page, as account URLs in the order they were reported.
///
/// Also asserts the paging invariants the rule names: no page is longer than the limit,
/// a `NextToken` is offered only beside a page that filled the limit (a short page has
/// nothing left to resume from), and the walk terminates.
async fn list_all(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_arn: &str,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut next: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({ "AcmeEndpointArn": endpoint_arn });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next.as_ref() {
            body["NextToken"] = json!(token);
        }
        let response = list_page(router, case, &body).await;
        let page = accounts_of(case, &response);
        let page_len = i64::try_from(page.len()).unwrap_or(i64::MAX);
        if let Some(max) = max_results {
            assert!(
                page_len <= max,
                "case {case:?}: a page must hold at most MaxResults={max} accounts, got {page_len}"
            );
        }
        for summary in &page {
            urls.push(
                summary["AccountUrl"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a summary carried no AccountUrl"))
                    .to_string(),
            );
        }
        next = response["NextToken"].as_str().map(str::to_string);
        if next.is_some() {
            let limit = max_results.unwrap_or(page_len);
            assert_eq!(
                page_len, limit,
                "case {case:?}: a NextToken may only follow a page that filled the limit \
                 {limit}, got a page of {page_len}"
            );
        }
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: paging must terminate, but a 65th page was still offered"
        );
        if next.is_none() {
            break;
        }
    }
    urls
}

/// RULE: every member of an `AcmeAccountSummary` is read off the stored account record
/// — for every row of the table, spanning each `AcmeAccountStatus`, both presences of
/// an external account binding and contact lists of several sizes and scripts.
#[tokio::test]
async fn list_reports_every_member_of_the_stored_account() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "projection").await;
    let records = register_accounts(&engine, &endpoint_arn, &account_cases());

    let response = list_page(
        &router,
        "projection",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    let reported = accounts_of("projection", &response);
    assert_eq!(
        reported.len(),
        records.len(),
        "case \"projection\": every registered account must be listed, got {response}"
    );

    for (case, record) in account_cases().iter().zip(records.iter()) {
        let case = case.case;
        let summary = reported
            .iter()
            .find(|summary| summary["AccountUrl"].as_str() == Some(record.account_url.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} must be listed, got {reported:?}",
                    record.account_url
                )
            });
        assert_eq!(
            summary["PublicKeyThumbprint"].as_str(),
            Some(record.public_key_thumbprint.as_str()),
            "case {case:?}: expected the stored thumbprint {:?}, got {summary}",
            record.public_key_thumbprint
        );
        assert_eq!(
            summary["Status"].as_str(),
            Some(record.status.as_str()),
            "case {case:?}: expected the stored status {:?}, got {summary}",
            record.status
        );
        let expected_created = record
            .created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the seeded instant is after the epoch")
            .as_secs_f64();
        let reported_created = summary["CreatedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a CreatedAt timestamp, got {summary}")
        });
        assert!(
            (reported_created - expected_created).abs() < 0.001,
            "case {case:?}: expected CreatedAt {expected_created}, got {reported_created}"
        );
        assert_eq!(
            summary["AcmeExternalAccountBindingArn"].as_str(),
            record.acme_external_account_binding_arn.as_deref(),
            "case {case:?}: expected the stored binding ARN {:?}, got {summary}",
            record.acme_external_account_binding_arn
        );
        if record.contacts.is_empty() {
            assert!(
                summary["Contacts"].is_null(),
                "case {case:?}: an account that registered no contacts must report none, \
                 got {summary}"
            );
        } else {
            let reported_contacts: Vec<&str> = summary["Contacts"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Contacts list in {summary}"))
                .iter()
                .map(|contact| contact.as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                reported_contacts, record.contacts,
                "case {case:?}: expected the stored contacts {:?}, got {reported_contacts:?}",
                record.contacts
            );
        }
    }
}

/// RULE: the accounts reported are the ones registered with the endpoint the request
/// NAMES — an endpoint's accounts never leak into another's, and an endpoint nobody has
/// registered against is listed as empty rather than refused.
#[tokio::test]
async fn list_is_scoped_to_the_endpoint_the_request_names() {
    let (router, engine) = router_with_engine();
    let busy = create_endpoint(&router, "endpoint with accounts").await;
    let quiet = create_endpoint(&router, "the other endpoint").await;
    let other_cases = vec![AccountCase {
        case: "the other endpoint's only account",
        account_id: "e5000000-0000-4000-8000-000000000009",
        status: ACCOUNT_STATUSES[0],
        with_binding: false,
        contacts: vec!["mailto:other@example.com"],
        registered_after: Duration::from_secs(7),
    }];
    let busy_records = register_accounts(&engine, &busy, &account_cases());
    let quiet_records = register_accounts(&engine, &quiet, &other_cases);

    for (case, arn, expected) in [
        ("endpoint with accounts", &busy, &busy_records),
        ("the other endpoint, with one account", &quiet, &quiet_records),
    ] {
        let listed = list_all(&router, case, arn, None).await;
        let expected_urls: Vec<String> = {
            let mut urls: Vec<String> = expected
                .iter()
                .map(|record| record.account_url.clone())
                .collect();
            urls.sort();
            urls
        };
        assert_eq!(
            listed, expected_urls,
            "case {case:?}: expected exactly {arn}'s accounts {expected_urls:?}, got {listed:?}"
        );
    }

    // An endpoint with no accounts at all: an empty list, not a refusal and not another
    // endpoint's accounts.
    let empty = create_endpoint(&router, "freshly created endpoint").await;
    let response = list_page(
        &router,
        "freshly created endpoint",
        &json!({ "AcmeEndpointArn": empty }),
    )
    .await;
    assert!(
        accounts_of("freshly created endpoint", &response).is_empty(),
        "case \"freshly created endpoint\": an endpoint nobody registered against must \
         list no accounts, got {response}"
    );
    assert!(
        response["NextToken"].is_null(),
        "case \"freshly created endpoint\": a complete page must carry no NextToken, \
         got {response}"
    );
}

/// RULE: paging cuts the SAME ordered set into pages of at most `MaxResults`, and a
/// `NextToken` resumes where the previous page stopped — for every page size from one
/// through one past the number of registered accounts, for the member's published
/// bounds, and for a request that names no `MaxResults` at all.
#[tokio::test]
async fn paging_walks_every_account_exactly_once_at_every_page_size() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "paging").await;
    let records = register_accounts(&engine, &endpoint_arn, &account_cases());
    let mut expected: Vec<String> = records
        .iter()
        .map(|record| record.account_url.clone())
        .collect();
    expected.sort();
    let count = i64::try_from(expected.len()).expect("the table is small");

    let mut page_sizes: Vec<Option<i64>> = (1..=count + 1).map(Some).collect();
    // Both published bounds of `MaxResults`, and the request that names none.
    page_sizes.push(Some(MAX_RESULTS_RANGE.0));
    page_sizes.push(Some(MAX_RESULTS_RANGE.1));
    page_sizes.push(None);

    for max_results in page_sizes {
        let case = format!("MaxResults={max_results:?}");
        let listed = list_all(&router, &case, &endpoint_arn, max_results).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: paging must report every account exactly once in account-URL \
             order, expected {expected:?}, got {listed:?}"
        );
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>().len(),
            listed.len(),
            "case {case:?}: no account may be reported twice, got {listed:?}"
        );
    }
}

/// RULE: a `MaxResults` outside the member's modelled range is refused as invalid
/// input, on each published bound's far side.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "max results range").await;
    register_accounts(&engine, &endpoint_arn, &account_cases());

    for max_results in [MAX_RESULTS_RANGE.0 - 1, MAX_RESULTS_RANGE.1 + 1] {
        let case = format!("MaxResults={max_results}");
        let (status, response) = call(
            &router,
            "ListAcmeAccounts",
            &json!({ "AcmeEndpointArn": endpoint_arn, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside {MAX_RESULTS_RANGE:?} must be refused, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}

/// RULE: an `AcmeEndpointArn` no stored endpoint answers to is a missing resource, for
/// every well-formed ARN shape — one never created, and one whose endpoint has been
/// removed from the store.
#[tokio::test]
async fn an_endpoint_that_does_not_exist_is_not_found() {
    let (router, engine) = router_with_engine();
    let removed = create_endpoint(&router, "removed endpoint").await;
    register_accounts(&engine, &removed, &account_cases());
    {
        // Deleting the endpoint takes its accounts with it, so the ARN names nothing.
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let mut store = store.lock();
        store.remove_acme_endpoint(&removed);
        assert!(
            store.acme_accounts_of(&removed).next().is_none(),
            "case \"removed endpoint\": removing an endpoint must drop its accounts"
        );
    }

    let never_created = state::acme_endpoint_arn(REGION, "11111111-2222-4333-8444-555555555555");
    let other_region = state::acme_endpoint_arn("eu-west-1", "66666666-7777-4888-8999-aaaaaaaaaaaa");

    for (case, arn) in [
        ("an endpoint that was never created", &never_created),
        ("an endpoint minted in another Region", &other_region),
        ("an endpoint that was deleted", &removed),
    ] {
        let (status, response) = call(
            &router,
            "ListAcmeAccounts",
            &json!({ "AcmeEndpointArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {arn} must be refused as not found, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            state::acme_endpoint_not_found_message(arn),
            "case {case:?}: expected this service's not-found sentence, got {response}"
        );
    }
}

/// RULE: a `NextToken` is read only against the endpoint it was issued for, and a token
/// this service did not issue is refused rather than answered with a page.
#[tokio::test]
async fn a_next_token_is_bound_to_the_endpoint_that_issued_it() {
    let (router, engine) = router_with_engine();
    let first = create_endpoint(&router, "token issuer").await;
    let second = create_endpoint(&router, "other endpoint").await;
    register_accounts(&engine, &first, &account_cases());
    register_accounts(&engine, &second, &account_cases());

    let response = list_page(
        &router,
        "token issuer",
        &json!({ "AcmeEndpointArn": first, "MaxResults": 1 }),
    )
    .await;
    let token = response["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a NextToken, got {response}"))
        .to_string();

    for (case, arn, token) in [
        ("a token issued for another endpoint", &second, token.clone()),
        ("a token this service never issued", &first, "not-a-token".to_string()),
        ("an empty-position token", &first, hex::encode("\u{1f}")),
    ] {
        let (status, response) = call(
            &router,
            "ListAcmeAccounts",
            &json!({ "AcmeEndpointArn": arn, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {token} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            state::INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: expected this service's invalid-token sentence, got {response}"
        );
    }
}

/// RULE: listing is a REPORT — repeating it answers the same accounts, in the same
/// order, with the same members, and leaves the store as it was.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "repeat").await;
    register_accounts(&engine, &endpoint_arn, &account_cases());

    let body = json!({ "AcmeEndpointArn": endpoint_arn });
    let first = list_page(&router, "repeat", &body).await;
    let second = list_page(&router, "repeat", &body).await;
    assert_eq!(
        first, second,
        "case \"repeat\": two identical reads must answer identically, got {first} then {second}"
    );

    // And the records behind them are untouched: same count, same statuses.
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let statuses: Vec<&str> = store
        .acme_accounts_of(&endpoint_arn)
        .map(|account| account.status.as_str())
        .collect();
    let mut expected: Vec<&str> = account_cases().iter().map(|case| case.status).collect();
    let mut ordered = statuses.clone();
    ordered.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        ordered, expected,
        "case \"repeat\": listing must not change any account's status, expected {expected:?}, \
         got {statuses:?}"
    );
}
