// SPDX-License-Identifier: CC0-1.0

//! The order `ListBillingAdjustmentRequests` resolves its `agreementId` and its
//! `nextToken` in, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the validation seam, the handler, the persisted state and
//! serialization — the same path a client takes. The wire helpers come from the
//! suites beside this one (`agreement_lifecycle`, `billing_adjustments`,
//! `error_bodies`), so there is one harness for the crate.
//!
//! The rule under test: a listing whose page belongs to one agreement resolves
//! the agreement its `agreementId` names BEFORE it interprets the `nextToken`
//! that resumes a page, so a call naming an agreement the store does not hold is
//! refused about `agreementId` whatever token it carries and whichever of the
//! listing's other filters accompany it — a token names a page OF a listing, and
//! there is no listing of an agreement that does not exist. Once the agreement
//! resolves, the token is judged exactly as before: it still belongs to the whole
//! query it was minted for, and to no other.
//!
//! The suite beside this one (`listing_resolution_order`) spans that rule across
//! the paginated reads of this crate. The rows here vary what it does not: WHICH
//! of this listing's own filters the unresolvable id is combined with, and WHICH
//! query the token the call carries was minted for — every member the page
//! token's scope is keyed by (`agreementId`, `status`, `catalog`,
//! `agreementType` and the two creation bounds) among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_over, call, emulator, invoiced_agreement, is_next_token, term, Answer,
};
use crate::billing_adjustments::{entry, submit};
use crate::error_bodies::{assert_input_fault, unresolvable_agreement_ids};

/// The one catalog AWS Marketplace exposes, which every agreement this service
/// holds was created in — spelled out here rather than imported so a handler
/// that admits another one fails this test instead of agreeing with it.
const CATALOG: &str = "AWSMarketplace";

/// The two `agreementType` values the model documents: "The type of agreement.
/// Values are `PurchaseAgreement` or `VendorInsightsAgreement`"
/// (`smithy_query(query="doc", shape="DescribeAgreementOutput",
/// member="agreementType")`). The first is the type the agreements this crate
/// mints carry; the second is a value the member admits and that selects nothing.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";
/// See [`AGREEMENT_TYPE`].
const OTHER_AGREEMENT_TYPE: &str = "VendorInsightsAgreement";

/// One value of `BillingAdjustmentStatus`
/// (`smithy_query(query="shape", shape="BillingAdjustmentStatus")`: COMPLETED |
/// PENDING | VALIDATION_FAILED), used as the `status` filter that keys a scope.
const A_STATUS: &str = "VALIDATION_FAILED";

/// The largest and the smallest page `MaxResults` admits (`@range(min: 1,
/// max: 50)`).
const LARGEST_PAGE: i32 = 50;
/// See [`LARGEST_PAGE`].
const SMALLEST_PAGE: i32 = 1;

/// A value of the SHAPE this service's page tokens have — a hexadecimal scope
/// digest followed by a hexadecimal offset, inside `NextToken`'s
/// `^[a-zA-Z0-9+/=_-]+$` and `@length(max: 8192)` — that belongs to no query
/// this service ever minted a token for, so it reaches the handler rather than
/// being refused at the boundary.
const NEVER_MINTED_TOKEN: &str = "0123456789abcdef00000002";

/// `NextToken`'s published upper length bound, 8192
/// (`smithy_query(query="shape", shape="NextToken")`): the longest value the
/// boundary admits, so the handler is what answers it.
const LONGEST_TOKEN_LEN: usize = 8192;

/// The `ValidationExceptionReason` for an `agreementId` naming no agreement this
/// service holds, which is what a client branches on.
const INVALID_AGREEMENT_ID: &str = "INVALID_AGREEMENT_ID";

/// The `ValidationExceptionReason` for a token naming no page of the listing
/// called.
const INVALID_NEXT_TOKEN: &str = "INVALID_NEXT_TOKEN";

async fn list(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListBillingAdjustmentRequests", request).await
}

/// The `billingAdjustmentRequestId` of every item a listing answered, in the
/// order it answered them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("items is a required member, got {:?}", answer.body))
        .iter()
        .map(|item| {
            item["billingAdjustmentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One request body with `agreementId` set to `agreement` and the companion
/// members and token the case names.
fn request(agreement: &str, companions: &Value, token: Option<&str>) -> Value {
    let mut body = companions.clone();
    body["agreementId"] = json!(agreement);
    if let Some(token) = token {
        body["nextToken"] = json!(token);
    }
    body
}

/// The token `operation` mints over `query`, asserted to be one.
async fn minted(router: &MultiServiceRouter, operation: &str, query: Value) -> String {
    let mut body = query;
    body["maxResults"] = json!(SMALLEST_PAGE);
    let answer = call(router, operation, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "minting a token with {operation} {body} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let token = answer.str_at("/nextToken");
    assert!(
        is_next_token(&token),
        "{operation} {body} must leave a token over the records it holds, got {:?}",
        answer.body
    );
    token
}

/// Store three billing adjustment requests against one invoiced agreement,
/// answering the agreement id and the created ids in creation order.
async fn adjusted_agreement(router: &MultiServiceRouter, proposal: &str) -> (String, Vec<String>) {
    let (agreement, invoice) = invoiced_agreement(router, proposal).await;
    let entries: Vec<Value> = (0..3)
        .map(|index| {
            entry(
                &agreement,
                &invoice,
                "1",
                &format!("tok-{proposal}-{index}"),
            )
        })
        .collect();
    let answer = submit(router, entries).await;
    assert_eq!(
        answer.status, 200,
        "submitting three adjustment requests against {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let ids = listed_ids(&answer);
    assert_eq!(
        ids.len(),
        3,
        "case {proposal:?}: one item per accepted entry, got {:?}",
        answer.body
    );
    (agreement, ids)
}

/// An `agreementId` naming no stored agreement is answered about `agreementId`,
/// whatever filters and whatever page token the call carries beside it.
#[tokio::test]
async fn an_unknown_agreement_is_refused_before_any_page_token_is_read() {
    let router = emulator();

    // Two agreements with adjustment requests of their own, and one agreement
    // over two terms — so several different queries of this emulator can mint a
    // real token below.
    let (first, _) = adjusted_agreement(&router, "ap-orderfirst").await;
    let (second, _) = adjusted_agreement(&router, "ap-ordersecond").await;
    let termed = agreement_over(
        &router,
        "ap-orderterms",
        vec![term("eula.v1"), term("SupportTerm")],
    )
    .await;

    // One token per scope member this listing keys a page by, so no row relies
    // on the shape of a token the service never minted.
    let unfiltered = minted(&router, "ListBillingAdjustmentRequests", json!({})).await;
    let over_first = minted(
        &router,
        "ListBillingAdjustmentRequests",
        json!({"agreementId": first}),
    )
    .await;
    let over_catalog = minted(
        &router,
        "ListBillingAdjustmentRequests",
        json!({"catalog": CATALOG, "agreementType": AGREEMENT_TYPE}),
    )
    .await;
    let over_window = minted(
        &router,
        "ListBillingAdjustmentRequests",
        json!({"createdAfter": 1.0, "createdBefore": 4_102_444_800.0}),
    )
    .await;
    let sibling = minted(&router, "GetAgreementTerms", json!({"agreementId": termed})).await;
    let longest = "a".repeat(LONGEST_TOKEN_LEN);

    // Which of this listing's own filters accompany the id that names no
    // agreement. Each value is one this service admits, so the only thing wrong
    // with any of these calls is the agreement.
    let companions: Vec<(&str, Value)> = vec![
        ("the id alone", json!({})),
        ("beside a status filter", json!({"status": A_STATUS})),
        (
            "beside the catalog this service exposes",
            json!({"catalog": CATALOG}),
        ),
        (
            "beside the agreementType its agreements carry",
            json!({"agreementType": AGREEMENT_TYPE}),
        ),
        (
            "beside the other agreementType the member admits",
            json!({"agreementType": OTHER_AGREEMENT_TYPE}),
        ),
        (
            "beside both creation bounds",
            json!({"createdAfter": 1.0, "createdBefore": 4_102_444_800.0}),
        ),
        (
            "beside the largest page the model admits",
            json!({"maxResults": LARGEST_PAGE}),
        ),
        (
            "beside the smallest page the model admits",
            json!({"maxResults": SMALLEST_PAGE}),
        ),
        (
            "beside every filter at once",
            json!({
                "status": A_STATUS,
                "catalog": CATALOG,
                "agreementType": AGREEMENT_TYPE,
                "createdAfter": 1.0,
                "createdBefore": 4_102_444_800.0,
                "maxResults": SMALLEST_PAGE,
            }),
        ),
    ];

    // Where the token the call carries came from: none at all, one of the right
    // shape this service never minted, one this listing minted over each of the
    // scopes above, one another read minted, and one at NextToken's published
    // length bound.
    let tokens: Vec<(&str, Option<&str>)> = vec![
        ("no token at all", None),
        (
            "a well-formed token this service never minted",
            Some(NEVER_MINTED_TOKEN),
        ),
        (
            "a token this listing minted over every stored request",
            Some(unfiltered.as_str()),
        ),
        (
            "a token this listing minted over an agreement that exists",
            Some(over_first.as_str()),
        ),
        (
            "a token this listing minted over a catalog and type filter",
            Some(over_catalog.as_str()),
        ),
        (
            "a token this listing minted over a creation window",
            Some(over_window.as_str()),
        ),
        (
            "a token another read minted",
            Some(sibling.as_str()),
        ),
        (
            "a token at NextToken's published length bound",
            Some(longest.as_str()),
        ),
    ];

    for (id_case, absent) in unresolvable_agreement_ids() {
        for (companion_case, companion) in &companions {
            for (token_case, token) in &tokens {
                let case = format!("{id_case}, {companion_case}, with {token_case}");
                let answer = list(&router, request(&absent, companion, *token)).await;
                assert_input_fault(&case, INVALID_AGREEMENT_ID, "agreementId", &answer);
            }
        }
    }

    // The order of the two resolutions is all there is to it: for an agreement
    // that DOES exist, the token is judged exactly as before — it resumes the
    // query it was minted for, and nothing else.
    let resumed = list(
        &router,
        json!({"agreementId": first, "maxResults": SMALLEST_PAGE, "nextToken": over_first}),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case \"a token resuming the query it was minted for\": expected 200, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        listed_ids(&resumed).len(),
        1,
        "case \"a token resuming the query it was minted for\": expected the next page of one, \
         got {:?}",
        resumed.body
    );

    for (case, elsewhere) in [
        (
            "a token minted over one agreement cannot resume another that exists",
            json!({"agreementId": second, "nextToken": over_first}),
        ),
        (
            "a token minted over an agreement cannot resume it under a further filter",
            json!({"agreementId": first, "status": A_STATUS, "nextToken": over_first}),
        ),
        (
            "a token minted over every stored request cannot resume one agreement's",
            json!({"agreementId": first, "nextToken": unfiltered}),
        ),
        (
            "a token this service never minted names no page of an agreement that exists",
            json!({"agreementId": first, "nextToken": NEVER_MINTED_TOKEN}),
        ),
    ] {
        let answer = list(&router, elsewhere).await;
        assert_input_fault(case, INVALID_NEXT_TOKEN, "nextToken", &answer);
    }
}

/// Paging right through one agreement's requests still works: the order the two
/// resolutions happen in changes nothing about the pages themselves.
#[tokio::test]
async fn an_agreement_that_exists_still_pages_through_its_requests() {
    let router = emulator();
    let (agreement, created) = adjusted_agreement(&router, "ap-orderpages").await;

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 1..=created.len() {
        let answer = list(
            &router,
            request(
                &agreement,
                &json!({"maxResults": SMALLEST_PAGE}),
                token.as_deref(),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case page {page}: the listing must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        let ids = listed_ids(&answer);
        assert_eq!(
            ids.len(),
            1,
            "case page {page}: expected one item per page, got {:?}",
            answer.body
        );
        seen.extend(ids);
        token = answer.body["nextToken"].as_str().map(str::to_string);
        if page == created.len() {
            assert!(
                token.is_none(),
                "case page {page}: the last page carries no nextToken, got {token:?}"
            );
        } else {
            let minted = token
                .as_deref()
                .unwrap_or_else(|| panic!("case page {page}: a further page needs a nextToken"));
            assert!(
                is_next_token(minted),
                "case page {page}: a token must match NextToken's pattern, got {minted:?}"
            );
        }
    }
    assert_eq!(
        seen, created,
        "the pages together must report every request once, in creation order: expected \
         {created:?}, got {seen:?}"
    );
}
