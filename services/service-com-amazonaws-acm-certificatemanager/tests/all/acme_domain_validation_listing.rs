// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAcmeDomainValidations` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.ListAcmeDomainValidations`, awsJson1_1).
//!
//! RULE: `ListAcmeDomainValidations` REPORTS the domain validations the store holds for
//! the endpoint `AcmeEndpointArn` names — every member projected from the stored record,
//! exactly as `DescribeAcmeDomainValidation` projects the same record, in one
//! deterministic order, paged by `MaxResults` with a `NextToken` that resumes exactly
//! where the previous page stopped — answers `ResourceNotFoundException` for an ARN no
//! stored endpoint answers to, and creates, changes or consumes nothing.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every validation is created by
//! `CreateAcmeDomainValidation` and read back through `ListAcmeDomainValidations`, a
//! different operation, so a response that is not computed from the persisted store
//! cannot pass. The wire harness, the endpoint helper and the validation table are shared
//! with [`crate::certificate_lifecycle`], [`crate::acme_endpoint_lifecycle`] and
//! [`crate::acme_domain_validation_lifecycle`].
//!
//! The table spans the classes the rule generalises over: domain names of several forms
//! (apex, deep subdomain, punycode, single label, and one at `DomainName`'s modelled
//! maximum length), prevalidation options with and without a `DomainScope` and a
//! `HostedZoneId`, tag sets of several sizes and scripts, both statuses a stored
//! validation can be read in (`VALIDATING` while the prevalidation the clock owes is
//! outstanding, `VALID` once it has fallen due), page sizes from one through one past the
//! number of stored validations as well as both published `MaxResults` bounds and one
//! past each, and endpoints holding none, one and several validations.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body, hold_prevalidations,
    settle_prevalidations_now, tag_case, validation_cases, DOMAIN_SCOPE_OPTIONS, HOSTED_ZONE_ID,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{call, error_code, error_message, router_with_engine, REGION};

/// `MaxResults`' modelled range
/// (`smithy_query(query="operation", operation="ListAcmeDomainValidations")` reports
/// `range {min: 1, max: 100}` for that member).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// Create one domain validation per row of the shared table on `endpoint_arn`, returning
/// the ARNs the service minted, in the order the rows were created.
async fn create_table(router: &MultiServiceRouter, endpoint_arn: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for (index, (case, domain, options)) in validation_cases().into_iter().enumerate() {
        let mut body = json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": domain,
            "PrevalidationOptions": options,
        });
        if let Some(tags) = tag_case(index) {
            body["Tags"] = tags;
        }
        arns.push(create_validation(router, case, &body).await);
    }
    arns
}

/// One `ListAcmeDomainValidations` call, asserted to succeed, returning the response.
async fn list_page(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "ListAcmeDomainValidations", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeDomainValidations({body}) must succeed, got {status} {response}"
    );
    response
}

/// The `AcmeDomainValidations` member of a response, as a list.
fn validations_of(case: &str, response: &Value) -> Vec<Value> {
    response["AcmeDomainValidations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no AcmeDomainValidations list in {response}"))
        .clone()
}

/// Every summary `ListAcmeDomainValidations` reports for `endpoint_arn`, paging to the
/// end with `max_results` per page, in the order they were reported.
///
/// Also asserts the paging invariants the rule names: no page is longer than the limit, a
/// `NextToken` is offered only beside a page that filled the limit (a short page has
/// nothing left to resume from), and the walk terminates.
async fn list_all(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_arn: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut summaries = Vec::new();
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
        let page = validations_of(case, &response);
        let page_len = i64::try_from(page.len()).unwrap_or(i64::MAX);
        if let Some(max) = max_results {
            assert!(
                page_len <= max,
                "case {case:?}: a page must hold at most MaxResults={max} validations, \
                 got {page_len}"
            );
        }
        summaries.extend(page);
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
    summaries
}

/// The `AcmeDomainValidationArn` of each reported summary, in the order reported.
fn reported_arns(case: &str, summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["AcmeDomainValidationArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a summary carried no AcmeDomainValidationArn: {summary}")
                })
                .to_string()
        })
        .collect()
}

/// RULE: every member of an `AcmeDomainValidationSummary` is read off the stored record —
/// the same record `DescribeAcmeDomainValidation` reports, member for member — for every
/// row of the shared table.
#[tokio::test]
async fn list_reports_every_member_of_the_stored_validation() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "projection", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;
    // Pin the transient status: the prevalidation each create owes is pushed out of the
    // test's way, so `VALIDATING` cannot race the wall clock.
    hold_prevalidations(&engine);

    let summaries = list_all(&router, "projection", &endpoint_arn, None).await;
    assert_eq!(
        summaries.len(),
        created.len(),
        "case \"projection\": every created validation must be listed, got {summaries:?}"
    );

    for (case, domain, options) in validation_cases() {
        let summary = summaries
            .iter()
            .find(|summary| summary["DomainName"].as_str() == Some(domain.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {domain} must be listed, got {summaries:?}")
            });
        let arn = summary["AcmeDomainValidationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ARN on {summary}"));
        assert!(
            created.contains(&arn.to_string()),
            "case {case:?}: the listed ARN {arn} must be one CreateAcmeDomainValidation \
             minted, got {created:?}"
        );
        assert_eq!(
            summary["AcmeEndpointArn"].as_str(),
            Some(endpoint_arn.as_str()),
            "case {case:?}: expected the validation to report endpoint {endpoint_arn}, \
             got {summary}"
        );
        assert_eq!(
            summary["Status"].as_str(),
            Some(state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
            "case {case:?}: expected a freshly created validation to be {}, got {summary}",
            state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING
        );
        assert_eq!(
            summary["PrevalidationType"].as_str(),
            Some(state::PREVALIDATION_TYPE_DNS),
            "case {case:?}: expected PrevalidationType {}, got {summary}",
            state::PREVALIDATION_TYPE_DNS
        );
        // The prevalidation details report what the create configured, beside the DNS
        // record the SERVICE minted for this validation.
        let details = &summary["PrevalidationDetails"]["DnsPrevalidation"];
        let requested = &options["DnsPrevalidation"];
        assert_eq!(
            details["HostedZoneId"], requested["HostedZoneId"],
            "case {case:?}: expected the stored HostedZoneId to be reported, got {summary}"
        );
        assert_eq!(
            details["DomainScope"], requested["DomainScope"],
            "case {case:?}: expected the stored DomainScope to be reported, got {summary}"
        );
        assert!(
            details["ResourceRecord"]["Name"].is_string()
                && details["ResourceRecord"]["Type"].is_string()
                && details["ResourceRecord"]["Value"].is_string(),
            "case {case:?}: expected the server-managed resource record to be reported, \
             got {summary}"
        );
        assert!(
            summary["FailureDetails"].is_null(),
            "case {case:?}: nothing failed, so FailureDetails must be absent, got {summary}"
        );

        // The summary and the DETAIL project ONE record, so the listed validation and the
        // described one agree on every member the two shapes share.
        let detail = describe_validation(&router, case, arn).await;
        assert_eq!(
            *summary, detail,
            "case {case:?}: the listed summary and DescribeAcmeDomainValidation must report \
             the same record, got {summary} and {detail}"
        );
    }
}

/// RULE: the validations reported are the ones stored for the endpoint the request NAMES
/// — one endpoint's validations never leak into another's, and an endpoint that holds
/// none is listed as empty rather than refused.
#[tokio::test]
async fn list_is_scoped_to_the_endpoint_the_request_names() {
    let (router, engine) = router_with_engine();
    let busy = create_endpoint(&router, "endpoint with validations", &endpoint_body()).await;
    let one = create_endpoint(&router, "endpoint with one validation", &endpoint_body()).await;
    let empty = create_endpoint(&router, "endpoint with none", &endpoint_body()).await;

    let busy_arns = create_table(&router, &busy).await;
    // The second endpoint holds exactly one validation, on a domain and a configuration
    // the table's rows do not use.
    let lone = create_validation(
        &router,
        "endpoint with one validation",
        &json!({
            "AcmeEndpointArn": one,
            "DomainName": "solo.example.net",
            "PrevalidationOptions": {
                "DnsPrevalidation": {
                    "DomainScope": { "Subdomains": DOMAIN_SCOPE_OPTIONS[1] },
                    "HostedZoneId": HOSTED_ZONE_ID,
                },
            },
        }),
    )
    .await;
    hold_prevalidations(&engine);

    for (case, arn, expected) in [
        ("endpoint with validations", &busy, busy_arns.clone()),
        ("endpoint with one validation", &one, vec![lone.clone()]),
        ("endpoint with none", &empty, Vec::new()),
    ] {
        let summaries = list_all(&router, case, arn, None).await;
        let listed = reported_arns(case, &summaries);
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            listed, expected_sorted,
            "case {case:?}: expected exactly {arn}'s validations {expected_sorted:?}, \
             got {listed:?}"
        );
    }

    // An endpoint with no validations answers an empty list and offers no token.
    let response = list_page(&router, "endpoint with none", &json!({ "AcmeEndpointArn": empty })).await;
    assert!(
        validations_of("endpoint with none", &response).is_empty(),
        "case \"endpoint with none\": an endpoint with no validations must list none, \
         got {response}"
    );
    assert!(
        response["NextToken"].is_null(),
        "case \"endpoint with none\": a complete page must carry no NextToken, got {response}"
    );
}

/// RULE: paging cuts the SAME ordered set into pages of at most `MaxResults`, and a
/// `NextToken` resumes where the previous page stopped — for every page size from one
/// through one past the number of stored validations, for the member's published bounds,
/// and for a request that names no `MaxResults` at all.
#[tokio::test]
async fn paging_walks_every_validation_exactly_once_at_every_page_size() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "paging", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;
    hold_prevalidations(&engine);

    let mut expected = created.clone();
    expected.sort();
    let count = i64::try_from(expected.len()).expect("the table is small");

    let mut page_sizes: Vec<Option<i64>> = (1..=count + 1).map(Some).collect();
    // Both published bounds of `MaxResults`, and the request that names none.
    page_sizes.push(Some(MAX_RESULTS_RANGE.0));
    page_sizes.push(Some(MAX_RESULTS_RANGE.1));
    page_sizes.push(None);

    for max_results in page_sizes {
        let case = format!("MaxResults={max_results:?}");
        let summaries = list_all(&router, &case, &endpoint_arn, max_results).await;
        let listed = reported_arns(&case, &summaries);
        assert_eq!(
            listed, expected,
            "case {case:?}: paging must report every validation exactly once in ARN order, \
             expected {expected:?}, got {listed:?}"
        );
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>().len(),
            listed.len(),
            "case {case:?}: no validation may be reported twice, got {listed:?}"
        );
    }
}

/// RULE: a `MaxResults` outside the member's modelled range is refused as invalid input,
/// on each published bound's far side.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let (router, _engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "max results range", &endpoint_body()).await;
    create_table(&router, &endpoint_arn).await;

    for max_results in [MAX_RESULTS_RANGE.0 - 1, MAX_RESULTS_RANGE.1 + 1] {
        let case = format!("MaxResults={max_results}");
        let (status, response) = call(
            &router,
            "ListAcmeDomainValidations",
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
/// every well-formed ARN shape — one never created, one minted in another Region, and one
/// whose endpoint (and therefore its validations) has been removed from the store.
#[tokio::test]
async fn an_endpoint_that_does_not_exist_is_not_found() {
    let (router, engine) = router_with_engine();
    let removed = create_endpoint(&router, "removed endpoint", &endpoint_body()).await;
    create_table(&router, &removed).await;
    {
        // Deleting the endpoint takes its validations with it, so the ARN names nothing.
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let mut store = store.lock();
        store.remove_acme_endpoint(&removed);
        assert!(
            store.acme_domain_validations_of(&removed).next().is_none(),
            "case \"removed endpoint\": removing an endpoint must drop its validations"
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
            "ListAcmeDomainValidations",
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
    let first = create_endpoint(&router, "token issuer", &endpoint_body()).await;
    let second = create_endpoint(&router, "other endpoint", &endpoint_body()).await;
    create_table(&router, &first).await;
    create_table(&router, &second).await;
    hold_prevalidations(&engine);

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
        (
            "a token this service never issued",
            &first,
            "not-a-token".to_string(),
        ),
        ("an empty-position token", &first, hex::encode("\u{1f}")),
    ] {
        let (status, response) = call(
            &router,
            "ListAcmeDomainValidations",
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

/// RULE: the status a listed validation carries is the one the CLOCK owes it, the same
/// status `DescribeAcmeDomainValidation` reports — `VALIDATING` while the prevalidation is
/// outstanding and `VALID` once it has fallen due — and listing is what reports it, never
/// what causes it.
#[tokio::test]
async fn listing_reports_the_status_the_clock_owes() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "status", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;

    hold_prevalidations(&engine);
    for (case, expected) in [
        (
            "prevalidation still outstanding",
            state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING,
        ),
        (
            "prevalidation already due",
            state::ACME_DOMAIN_VALIDATION_STATUS_VALID,
        ),
    ] {
        if expected == state::ACME_DOMAIN_VALIDATION_STATUS_VALID {
            settle_prevalidations_now(&engine);
        }
        let summaries = list_all(&router, case, &endpoint_arn, Some(2)).await;
        assert_eq!(
            summaries.len(),
            created.len(),
            "case {case:?}: every created validation must still be listed, got {summaries:?}"
        );
        for summary in &summaries {
            assert_eq!(
                summary["Status"].as_str(),
                Some(expected),
                "case {case:?}: expected status {expected}, got {summary}"
            );
            let arn = summary["AcmeDomainValidationArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ARN on {summary}"));
            let detail = describe_validation(&router, case, arn).await;
            assert_eq!(
                detail["Status"].as_str(),
                Some(expected),
                "case {case:?}: the describe of {arn} must agree with the listing, got {detail}"
            );
        }
    }
}

/// RULE: listing is a REPORT — repeating it answers the same validations, in the same
/// order, with the same members, and leaves the stored records as they were.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "repeat", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;
    hold_prevalidations(&engine);

    let body = json!({ "AcmeEndpointArn": endpoint_arn });
    let first = list_page(&router, "repeat", &body).await;
    let second = list_page(&router, "repeat", &body).await;
    assert_eq!(
        first, second,
        "case \"repeat\": two identical reads must answer identically, got {first} then {second}"
    );

    // And the records behind them are untouched: the same ARNs, each still in the status
    // its create left it in, and no validation added or dropped.
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let stored: Vec<String> = store
        .acme_domain_validations_of(&endpoint_arn)
        .map(|validation| validation.arn.clone())
        .collect();
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        stored, expected,
        "case \"repeat\": listing must neither add nor drop a validation, expected \
         {expected:?}, got {stored:?}"
    );
    let statuses: Vec<&str> = store
        .acme_domain_validations_of(&endpoint_arn)
        .map(|validation| validation.status.as_str())
        .collect();
    assert!(
        statuses
            .iter()
            .all(|status| *status == state::ACME_DOMAIN_VALIDATION_STATUS_VALIDATING),
        "case \"repeat\": listing must not move a held validation's status, got {statuses:?}"
    );
}
