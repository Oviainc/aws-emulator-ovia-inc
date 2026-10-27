// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAcmeExternalAccountBindings` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.ListAcmeExternalAccountBindings`, awsJson1_1).
//!
//! RULE: `ListAcmeExternalAccountBindings` REPORTS the external account bindings the
//! store holds for the endpoint `AcmeEndpointArn` names — every member projected from the
//! stored record, exactly as `DescribeAcmeExternalAccountBinding` projects the same
//! record, in one deterministic order, paged by `MaxResults` with a `NextToken` that
//! resumes exactly where the previous page stopped — answers `ResourceNotFoundException`
//! for an ARN no stored endpoint answers to, and creates, changes or consumes nothing
//! (not even a binding's credentials, which are no members of the listed shape).
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every binding is created by
//! `CreateAcmeExternalAccountBinding` and read back through
//! `ListAcmeExternalAccountBindings`, a different operation, so a response that is not
//! computed from the persisted store cannot pass. The wire harness, the endpoint helper
//! and the binding table are shared with [`crate::certificate_lifecycle`],
//! [`crate::acme_endpoint_lifecycle`] and
//! [`crate::acme_external_account_binding_lifecycle`].
//!
//! The table spans the classes the rule generalises over: every `TimeType` unit an
//! `Expiration` can be counted in and a create that names none, `Expiration$Value` on its
//! modelled lower bound and far above it, role ARNs of several forms, tag sets of several
//! sizes and scripts, bindings that have been revoked beside bindings that have not, page
//! sizes from one through one past the number of stored bindings as well as both published
//! `MaxResults` bounds and one past each, and endpoints holding none, one and several
//! bindings.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::{
    binding_body, binding_cases, create_binding, describe_binding, stored_credentials,
};
use crate::acme_external_account_binding_revocation::revoke_binding;
use crate::certificate_lifecycle::{call, error_code, error_message, router_with_engine, REGION};

/// `MaxResults`' modelled range
/// (`smithy_query(query="operation", operation="ListAcmeExternalAccountBindings")` reports
/// `range {min: 1, max: 100}` for that member).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// Create one external account binding per row of the shared table on `endpoint_arn`,
/// returning the ARNs the service minted, in the order the rows were created.
async fn create_table(router: &MultiServiceRouter, endpoint_arn: &str) -> Vec<String> {
    let mut arns = Vec::new();
    for case in binding_cases() {
        let (what, _expiration, _role_arn, _tags) = case.clone();
        let created = create_binding(router, what, &binding_body(&case, endpoint_arn)).await;
        arns.push(
            created["AcmeExternalAccountBindingArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN: {created}"))
                .to_string(),
        );
    }
    arns
}

/// One `ListAcmeExternalAccountBindings` call, asserted to succeed, returning the
/// response.
async fn list_page(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "ListAcmeExternalAccountBindings", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeExternalAccountBindings({body}) must succeed, \
         got {status} {response}"
    );
    response
}

/// The `ExternalAccountBindings` member of a response, as a list.
fn bindings_of(case: &str, response: &Value) -> Vec<Value> {
    response["ExternalAccountBindings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ExternalAccountBindings list in {response}"))
        .clone()
}

/// Every summary `ListAcmeExternalAccountBindings` reports for `endpoint_arn`, paging to
/// the end with `max_results` per page, in the order they were reported.
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
        let page = bindings_of(case, &response);
        let page_len = i64::try_from(page.len()).unwrap_or(i64::MAX);
        if let Some(max) = max_results {
            assert!(
                page_len <= max,
                "case {case:?}: a page must hold at most MaxResults={max} bindings, \
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

/// The `AcmeExternalAccountBindingArn` of each reported summary, in the order reported.
fn reported_arns(case: &str, summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["AcmeExternalAccountBindingArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a summary carried no AcmeExternalAccountBindingArn: \
                         {summary}"
                    )
                })
                .to_string()
        })
        .collect()
}

/// RULE: every member of an `AcmeExternalAccountBindingSummary` is read off the stored
/// record — the same record `DescribeAcmeExternalAccountBinding` reports, member for
/// member — for every row of the shared table, and the credentials the record carries are
/// reported by neither shape.
#[tokio::test]
async fn list_reports_every_member_of_the_stored_binding() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "binding host", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;

    let summaries = list_all(&router, "projection", &endpoint_arn, None).await;
    assert_eq!(
        summaries.len(),
        created.len(),
        "case \"projection\": every created binding must be listed, got {summaries:?}"
    );

    for (index, case) in binding_cases().into_iter().enumerate() {
        let (what, expiration, role_arn, _tags) = case;
        let arn = &created[index];
        let summary = summaries
            .iter()
            .find(|summary| summary["AcmeExternalAccountBindingArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {what:?}: {arn} must be listed, got {summaries:?}"));

        assert_eq!(
            summary["AcmeEndpointArn"].as_str(),
            Some(endpoint_arn.as_str()),
            "case {what:?}: expected the binding to report endpoint {endpoint_arn}, got {summary}"
        );
        assert_eq!(
            summary["RoleArn"].as_str(),
            Some(role_arn.as_str()),
            "case {what:?}: expected the stored RoleArn {role_arn}, got {summary}"
        );
        assert!(
            summary["RevokedAt"].is_null() && summary["LastUsedAt"].is_null(),
            "case {what:?}: nothing revoked the binding and nothing used it, so neither \
             instant may be reported, got {summary}"
        );
        assert_eq!(
            summary["ExpiresAt"].is_null(),
            expiration.is_none(),
            "case {what:?}: ExpiresAt must be reported exactly for a create that named an \
             Expiration, got {summary}"
        );

        // The summary and the DETAIL project ONE record, so the listed binding and the
        // described one agree on every member the two shapes share.
        let detail = describe_binding(&router, what, arn).await;
        assert_eq!(
            *summary, detail,
            "case {what:?}: the listed summary and DescribeAcmeExternalAccountBinding must \
             report the same record, got {summary} and {detail}"
        );

        // The credentials are no members of the listed shape: a listing hands over no
        // secret, and reading the list does not re-mint one.
        let (key_id, mac_key) = stored_credentials(&engine, arn);
        let rendered = summary.to_string();
        assert!(
            !rendered.contains(&mac_key),
            "case {what:?}: a listed summary must carry no MAC key, got {summary}"
        );
        list_all(&router, what, &endpoint_arn, Some(1)).await;
        assert_eq!(
            stored_credentials(&engine, arn),
            (key_id, mac_key),
            "case {what:?}: listing must not re-mint the binding's credentials"
        );
    }
}

/// RULE: the bindings reported are the ones stored for the endpoint the request NAMES —
/// one endpoint's bindings never leak into another's, and an endpoint that holds none is
/// listed as empty rather than refused.
#[tokio::test]
async fn list_is_scoped_to_the_endpoint_the_request_names() {
    let (router, _engine) = router_with_engine();
    let busy = create_endpoint(&router, "endpoint with bindings", &endpoint_body()).await;
    let one = create_endpoint(&router, "endpoint with one binding", &endpoint_body()).await;
    let empty = create_endpoint(&router, "endpoint with none", &endpoint_body()).await;

    let busy_arns = create_table(&router, &busy).await;
    // The second endpoint holds exactly one binding, on a role the table's rows do not
    // use.
    let lone_case = binding_cases()
        .into_iter()
        .next()
        .expect("the binding table is not empty");
    let lone = create_binding(
        &router,
        "endpoint with one binding",
        &binding_body(&lone_case, &one),
    )
    .await;
    let lone = lone["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, arn, expected) in [
        ("endpoint with bindings", &busy, busy_arns.clone()),
        ("endpoint with one binding", &one, vec![lone.clone()]),
        ("endpoint with none", &empty, Vec::new()),
    ] {
        let summaries = list_all(&router, case, arn, None).await;
        let listed = reported_arns(case, &summaries);
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            listed, expected_sorted,
            "case {case:?}: expected exactly {arn}'s bindings {expected_sorted:?}, got {listed:?}"
        );
    }

    // An endpoint with no bindings answers an empty list and offers no token.
    let response = list_page(
        &router,
        "endpoint with none",
        &json!({ "AcmeEndpointArn": empty }),
    )
    .await;
    assert!(
        bindings_of("endpoint with none", &response).is_empty(),
        "case \"endpoint with none\": an endpoint with no bindings must list none, got {response}"
    );
    assert!(
        response["NextToken"].is_null(),
        "case \"endpoint with none\": a complete page must carry no NextToken, got {response}"
    );
}

/// RULE: paging cuts the SAME ordered set into pages of at most `MaxResults`, and a
/// `NextToken` resumes where the previous page stopped — for every page size from one
/// through one past the number of stored bindings, for the member's published bounds, and
/// for a request that names no `MaxResults` at all.
#[tokio::test]
async fn paging_walks_every_binding_exactly_once_at_every_page_size() {
    let (router, _engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "paging", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;

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
            "case {case:?}: paging must report every binding exactly once in ARN order, \
             expected {expected:?}, got {listed:?}"
        );
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>().len(),
            listed.len(),
            "case {case:?}: no binding may be reported twice, got {listed:?}"
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
            "ListAcmeExternalAccountBindings",
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
/// whose endpoint (and therefore its bindings) has been removed from the store — while an
/// ARN naming ANOTHER account's endpoint is a fact about the request, refused as invalid
/// input.
#[tokio::test]
async fn an_endpoint_that_does_not_exist_is_not_found() {
    let (router, engine) = router_with_engine();
    let removed = create_endpoint(&router, "removed endpoint", &endpoint_body()).await;
    create_table(&router, &removed).await;
    {
        // Deleting the endpoint takes its bindings with it, so the ARN names nothing.
        let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
        let mut store = store.lock();
        store.remove_acme_endpoint(&removed);
        assert!(
            store
                .acme_external_account_bindings_of(&removed)
                .next()
                .is_none(),
            "case \"removed endpoint\": removing an endpoint must drop its bindings"
        );
    }

    let never_created = state::acme_endpoint_arn(REGION, "11111111-2222-4333-8444-555555555555");
    let other_region =
        state::acme_endpoint_arn("eu-west-1", "66666666-7777-4888-8999-aaaaaaaaaaaa");

    for (case, arn) in [
        ("an endpoint that was never created", &never_created),
        ("an endpoint minted in another Region", &other_region),
        ("an endpoint that was deleted", &removed),
    ] {
        let (status, response) = call(
            &router,
            "ListAcmeExternalAccountBindings",
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

    // An endpoint ARN of ANOTHER account, in each Region shape above: a verdict on the
    // request, so it is reported as invalid input rather than as a resource this account
    // happens not to hold.
    for region in [REGION, "eu-west-1"] {
        let case = format!("another account's endpoint in {region}");
        let foreign = format!(
            "arn:aws:acm:{region}:210987654321:acme-endpoint/\
             77777777-8888-4999-8aaa-bbbbbbbbbbbb"
        );
        let (status, response) = call(
            &router,
            "ListAcmeExternalAccountBindings",
            &json!({ "AcmeEndpointArn": foreign }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {foreign} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}

/// RULE: a `NextToken` is read only against the endpoint it was issued for, and a token
/// this service did not issue is refused rather than answered with a page.
#[tokio::test]
async fn a_next_token_is_bound_to_the_endpoint_that_issued_it() {
    let (router, _engine) = router_with_engine();
    let first = create_endpoint(&router, "token issuer", &endpoint_body()).await;
    let second = create_endpoint(&router, "other endpoint", &endpoint_body()).await;
    create_table(&router, &first).await;
    create_table(&router, &second).await;

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
            "ListAcmeExternalAccountBindings",
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

/// RULE: a listing reports whatever the stored record now holds, so a revocation recorded
/// on one binding shows up in the listing of that binding and of no other, with the same
/// instant `DescribeAcmeExternalAccountBinding` reports — over one, several and all of the
/// endpoint's bindings.
#[tokio::test]
async fn listing_reports_the_revocations_the_store_holds() {
    let (router, _engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "revocation", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;

    let mut revoked: Vec<String> = Vec::new();
    for arn in &created {
        revoke_binding(&router, "revocation", arn).await;
        revoked.push(arn.clone());
        let case = format!("{} of {} bindings revoked", revoked.len(), created.len());

        let summaries = list_all(&router, &case, &endpoint_arn, Some(2)).await;
        assert_eq!(
            summaries.len(),
            created.len(),
            "case {case:?}: every binding must still be listed, got {summaries:?}"
        );
        for summary in &summaries {
            let listed = summary["AcmeExternalAccountBindingArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ARN on {summary}"));
            let expected_revoked = revoked.iter().any(|arn| arn == listed);
            assert_eq!(
                summary["RevokedAt"].is_null(),
                !expected_revoked,
                "case {case:?}: expected RevokedAt to be reported exactly for a revoked \
                 binding, got {summary}"
            );
            let detail = describe_binding(&router, &case, listed).await;
            assert_eq!(
                summary["RevokedAt"], detail["RevokedAt"],
                "case {case:?}: the describe of {listed} must report the same revocation \
                 instant as the listing, got {summary} and {detail}"
            );
        }
    }
}

/// RULE: listing is a REPORT — repeating it answers the same bindings, in the same order,
/// with the same members, and leaves the stored records as they were.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "repeat", &endpoint_body()).await;
    let created = create_table(&router, &endpoint_arn).await;

    let body = json!({ "AcmeEndpointArn": endpoint_arn });
    let first = list_page(&router, "repeat", &body).await;
    let second = list_page(&router, "repeat", &body).await;
    assert_eq!(
        first, second,
        "case \"repeat\": two identical reads must answer identically, got {first} then {second}"
    );

    // And the records behind them are untouched: the same ARNs, none revoked or used by
    // the reading, and no binding added or dropped.
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let stored: Vec<String> = store
        .acme_external_account_bindings_of(&endpoint_arn)
        .map(|binding| binding.arn.clone())
        .collect();
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        stored, expected,
        "case \"repeat\": listing must neither add nor drop a binding, expected {expected:?}, \
         got {stored:?}"
    );
    assert!(
        store
            .acme_external_account_bindings_of(&endpoint_arn)
            .all(|binding| binding.revoked_at.is_none() && binding.last_used_at.is_none()),
        "case \"repeat\": listing must neither revoke a binding nor mark it used"
    );
}
