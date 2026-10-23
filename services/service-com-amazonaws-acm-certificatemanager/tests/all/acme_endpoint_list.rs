// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAcmeEndpoints` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.ListAcmeEndpoints`, awsJson1_1).
//!
//! RULE: `ListAcmeEndpoints` reports one `AcmeEndpointSummary` per ACME endpoint record
//! the account holds — projected from the same stored record `DescribeAcmeEndpoint`
//! reports, in one deterministic order — bounded by `MaxResults`, with a `NextToken`
//! that resumes strictly after the last endpoint the previous page emitted and is
//! issued only while endpoints are left beyond that page.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every listed endpoint is read back
//! through `DescribeAcmeEndpoint` — a DIFFERENT operation than the one that wrote it —
//! so a response that is not computed from the persisted store cannot pass.
//!
//! The tables below span the classes the rule generalises over: every create
//! configuration [`crate::acme_endpoint_lifecycle::endpoint_cases`] covers, both
//! statuses an endpoint can be listed in, every page size from one row to the model's
//! own `MaxResults` maximum (and one value past each published bound), and tokens of
//! several malformed shapes.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_endpoint_lifecycle::{
    create_endpoint, describe_endpoint, endpoint_cases, hold_activations, restore_activations,
    wait_until, AUTHORIZATION_BEHAVIORS, CONTACTS, PUBLIC_KEY_ALGORITHMS,
};
use crate::certificate_lifecycle::{call, error_code, router, router_with_engine};

/// `MaxResults`' modelled bounds
/// (`smithy_query(query="operation", operation="ListAcmeEndpoints")`: `range 1..100`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// One `ListAcmeEndpoints` page: its summaries and the `NextToken` it carried, asserted
/// to have succeeded.
async fn list_page(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
) -> (Vec<Value>, Option<String>) {
    let (status, response) = call(router, "ListAcmeEndpoints", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeEndpoints({body}) must succeed, got {status} {response}"
    );
    let summaries = response["AcmeEndpoints"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an AcmeEndpoints list, got {response}")
        })
        .clone();
    let next_token = response["NextToken"].as_str().map(str::to_string);
    (summaries, next_token)
}

/// Every summary `ListAcmeEndpoints` reports, paging to the end with `max_results` per
/// page (the server-side default when `None`), asserting that no page exceeds it.
async fn list_all(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next.as_ref() {
            body["NextToken"] = json!(token);
        }
        let (page, token) = list_page(router, case, &body).await;
        if let Some(max) = max_results {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max,
                "case {case:?}: a page may not exceed MaxResults={max}, got {} summaries",
                page.len()
            );
        }
        summaries.extend(page);
        next = token;
        if next.is_none() {
            break;
        }
        assert!(
            summaries.len() <= 256,
            "case {case:?}: paging did not terminate after {} summaries",
            summaries.len()
        );
    }
    summaries
}

/// The `AcmeEndpointArn` of each summary, in the order they were reported.
fn arns(summaries: &[Value], case: &str) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["AcmeEndpointArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary carried no AcmeEndpointArn"))
                .to_string()
        })
        .collect()
}

/// Create one endpoint per row of [`endpoint_cases`], answering the ARNs in creation
/// order.
async fn create_every_case(router: &MultiServiceRouter) -> Vec<String> {
    let mut created = Vec::new();
    for (case, body, _) in endpoint_cases() {
        created.push(create_endpoint(router, case, &body).await);
    }
    created
}

/// RULE: every endpoint `CreateAcmeEndpoint` persisted is listed exactly once, and each
/// summary is the record `DescribeAcmeEndpoint` reports for that ARN — member for
/// member, since `AcmeEndpointSummary` declares exactly the members `AcmeEndpoint` does
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeEndpointSummary")`).
#[tokio::test]
async fn list_reports_what_create_acme_endpoint_stored() {
    let (router, engine) = router_with_engine();
    let created = create_every_case(&router).await;
    // Pin the transient status, so the listed `CREATING` cannot race the wall clock.
    hold_activations(&engine);

    let case = "every create configuration";
    let summaries = list_all(&router, case, None).await;
    let listed = arns(&summaries, case);

    assert_eq!(
        listed.len(),
        created.len(),
        "case {case:?}: expected {} endpoints listed once each, got {listed:?}",
        created.len()
    );
    assert_eq!(
        listed.iter().cloned().collect::<BTreeSet<String>>(),
        created.iter().cloned().collect::<BTreeSet<String>>(),
        "case {case:?}: expected exactly the created endpoints to be listed, got {listed:?}"
    );

    for summary in &summaries {
        let arn = summary["AcmeEndpointArn"].as_str().unwrap_or_default();
        let described = describe_endpoint(&router, case, arn).await;
        assert_eq!(
            *summary, described,
            "case {case:?}: expected the summary of {arn} to report the stored record \
             DescribeAcmeEndpoint reports, got {summary} vs {described}"
        );
        assert_eq!(
            summary["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_CREATING),
            "case {case:?}: a freshly created endpoint must be listed as {}, got {summary}",
            state::ACME_ENDPOINT_STATUS_CREATING
        );
    }

    // Reading must not change what a later caller sees.
    let reread = list_all(&router, case, None).await;
    assert_eq!(
        reread, summaries,
        "case {case:?}: a second list must report the same endpoints, got {reread:?}"
    );
}

/// RULE: the listed `Status` is the one the clock has already made due — an endpoint
/// whose activation has fallen due is listed `ACTIVE`, and `DescribeAcmeEndpoint` agrees.
#[tokio::test]
async fn list_reports_the_status_the_clock_owes() {
    let (router, engine) = router_with_engine();
    // Three endpoints differing along the members the rule generalises over, so the
    // transition is observed on more than one configuration.
    let bodies = [
        json!({
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "Contact": CONTACTS[0],
            "CertificateAuthority": { "PublicCertificateAuthority": {} },
        }),
        json!({
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "Contact": CONTACTS[1],
            "CertificateAuthority": {
                "PublicCertificateAuthority": { "AllowedKeyAlgorithms": PUBLIC_KEY_ALGORITHMS },
            },
        }),
        json!({
            "AuthorizationBehavior": AUTHORIZATION_BEHAVIORS[0],
            "CertificateAuthority": {
                "PublicCertificateAuthority": { "AllowedKeyAlgorithms": ["RSA_2048"] },
            },
            "CertificateTags": [{ "Key": "環境", "Value": "本番" }],
        }),
    ];
    for (index, body) in bodies.iter().enumerate() {
        create_endpoint(&router, &format!("endpoint #{index}"), body).await;
    }

    hold_activations(&engine);
    let creating = list_all(&router, "activation held", None).await;
    for summary in &creating {
        assert_eq!(
            summary["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_CREATING),
            "case activation held: expected {}, got {summary}",
            state::ACME_ENDPOINT_STATUS_CREATING
        );
    }

    // Put the activations back where the creates fixed them and let that span pass.
    wait_until(restore_activations(&engine));
    let active = list_all(&router, "activation due", None).await;
    assert_eq!(
        arns(&active, "activation due"),
        arns(&creating, "activation held"),
        "case activation due: the same endpoints must be listed, got {active:?}"
    );
    for summary in &active {
        let arn = summary["AcmeEndpointArn"].as_str().unwrap_or_default();
        assert_eq!(
            summary["Status"].as_str(),
            Some(state::ACME_ENDPOINT_STATUS_ACTIVE),
            "case activation due: expected {} once the activation fell due, got {summary}",
            state::ACME_ENDPOINT_STATUS_ACTIVE
        );
        let described = describe_endpoint(&router, "activation due", arn).await;
        assert_eq!(
            *summary, described,
            "case activation due: list and describe must report one record for {arn}, \
             got {summary} vs {described}"
        );
    }
}

/// RULE: `MaxResults` bounds one page and `NextToken` resumes strictly after the last
/// endpoint the page emitted, so paging at ANY page size walks every endpoint exactly
/// once in the same order one unpaged call reports.
#[tokio::test]
async fn max_results_bounds_each_page_and_next_token_resumes() {
    let (router, engine) = router_with_engine();
    let created = create_every_case(&router).await;
    hold_activations(&engine);

    let unpaged = arns(&list_all(&router, "unpaged", None).await, "unpaged");
    assert_eq!(
        unpaged.len(),
        created.len(),
        "case unpaged: expected {} endpoints, got {unpaged:?}",
        created.len()
    );
    // The order is the store's own — one deterministic order, which is what makes a
    // resumed page neither repeat nor skip an endpoint.
    let mut sorted = unpaged.clone();
    sorted.sort();
    assert_eq!(
        unpaged, sorted,
        "case unpaged: expected one deterministic order, got {unpaged:?}"
    );

    // Page sizes spanning one row per page, several rows per page, exactly the number of
    // endpoints stored, one more than that, and both published bounds of the member.
    let sizes = [
        MAX_RESULTS_RANGE.0,
        2,
        3,
        i64::try_from(created.len()).unwrap_or(i64::MAX),
        i64::try_from(created.len() + 1).unwrap_or(i64::MAX),
        MAX_RESULTS_RANGE.1,
    ];
    for max in sizes {
        let case = format!("MaxResults={max}");
        let paged = arns(&list_all(&router, &case, Some(max)).await, &case);
        assert_eq!(
            paged, unpaged,
            "case {case}: paging must walk every endpoint exactly once in the unpaged \
             order, got {paged:?}"
        );
    }

    // A `NextToken` is issued only while endpoints are left beyond the page: the last
    // page of a complete walk carries none.
    let (page, token) = list_page(
        &router,
        "one page per endpoint",
        &json!({ "MaxResults": created.len() }),
    )
    .await;
    assert_eq!(
        page.len(),
        created.len(),
        "case one page per endpoint: expected every endpoint on one page, got {page:?}"
    );
    assert!(
        token.is_none(),
        "case one page per endpoint: a page that exhausted the store must carry no \
         NextToken, got {token:?}"
    );
    let (first, token) = list_page(&router, "first of two pages", &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        first.len(),
        1,
        "case first of two pages: expected one summary, got {first:?}"
    );
    assert!(
        token.is_some(),
        "case first of two pages: endpoints are left beyond the page, so a NextToken is \
         required"
    );
}

/// RULE: a `MaxResults` outside the bounds the model publishes is refused with HTTP 400
/// `ValidationException`, and each published bound itself is accepted.
#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let (router, engine) = router_with_engine();
    create_every_case(&router).await;
    hold_activations(&engine);

    for refused in [MAX_RESULTS_RANGE.0 - 1, MAX_RESULTS_RANGE.1 + 1] {
        let (status, response) = call(
            &router,
            "ListAcmeEndpoints",
            &json!({ "MaxResults": refused }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case MaxResults={refused}: expected 400 ValidationException for a value \
             outside the modelled range, got {status} {response}"
        );
    }

    for accepted in [MAX_RESULTS_RANGE.0, MAX_RESULTS_RANGE.1] {
        let case = format!("MaxResults={accepted}");
        let (page, _) = list_page(&router, &case, &json!({ "MaxResults": accepted })).await;
        assert!(
            !page.is_empty(),
            "case {case}: a bound of the modelled range must be accepted and report \
             endpoints, got {page:?}"
        );
    }
}

/// RULE: a `NextToken` this service did not issue is refused with HTTP 400
/// `ValidationException`, while a token naming an endpoint that has since been removed
/// still resumes after it — so a caller paging while the store changes keeps the tail.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = router_with_engine();
    let created = create_every_case(&router).await;
    hold_activations(&engine);

    // Tokens of several malformed shapes: not hex at all, odd-length hex, hex of text
    // that is no ARN, and hex of an ARN naming another ACM resource type.
    let forged = [
        ("not hex", "zzzz".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of plain text", hex::encode("not-an-arn")),
        (
            "hex of a certificate ARN",
            hex::encode(state::certificate_arn("us-west-2", "deadbeef")),
        ),
        (
            "hex of an ARN with no resource id",
            hex::encode(format!(
                "arn:aws:acm:us-west-2:{}:acme-endpoint/",
                engine::EMULATOR_ACCOUNT_ID
            )),
        ),
    ];
    for (case, token) in forged {
        let (status, response) = call(
            &router,
            "ListAcmeEndpoints",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for a token this service \
             did not issue, got {status} {response}"
        );
    }

    // A token the service DID issue, naming an endpoint that is gone by the time the
    // next page is asked for: the walk continues after it instead of failing or
    // restarting.
    let case = "cursor endpoint removed mid-walk";
    let (first, token) = list_page(&router, case, &json!({ "MaxResults": 1 })).await;
    let cursor_arn = arns(&first, case).remove(0);
    remove_endpoint(&engine, &cursor_arn);
    let token = token.unwrap_or_else(|| panic!("case {case:?}: the first page issued no token"));
    let (tail, _) = list_page(
        &router,
        case,
        &json!({ "MaxResults": created.len(), "NextToken": token }),
    )
    .await;
    let mut expected: Vec<String> = created
        .iter()
        .filter(|arn| **arn != cursor_arn)
        .cloned()
        .collect();
    expected.sort();
    assert_eq!(
        arns(&tail, case),
        expected,
        "case {case:?}: expected the walk to resume after {cursor_arn} with every \
         remaining endpoint, got {tail:?}"
    );
}

/// RULE: an account holding no ACME endpoint is reported as holding none — an empty
/// list and no `NextToken`, not a refusal and not some other account's endpoints.
#[tokio::test]
async fn an_account_with_no_endpoints_lists_none() {
    let router = router();
    for case in ["no MaxResults", "MaxResults at its minimum"] {
        let body = if case == "no MaxResults" {
            json!({})
        } else {
            json!({ "MaxResults": MAX_RESULTS_RANGE.0 })
        };
        let (page, token) = list_page(&router, case, &body).await;
        assert!(
            page.is_empty(),
            "case {case:?}: expected no endpoints, got {page:?}"
        );
        assert!(
            token.is_none(),
            "case {case:?}: nothing is left beyond an empty page, so no NextToken may be \
             issued, got {token:?}"
        );
    }
}

/// Drop one endpoint record from the store the way `DeleteAcmeEndpoint` will
/// ([`state::ComAmazonawsAcmCertificatemanagerState::remove_acme_endpoint`]), so a list
/// can be shown to report the store rather than anything it cached.
fn remove_endpoint(engine: &Arc<engine::InMemoryEngine>, arn: &str) {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let removed = store.lock().remove_acme_endpoint(arn);
    assert!(
        removed.is_some(),
        "the store must hold {arn} before it can be removed"
    );
}
