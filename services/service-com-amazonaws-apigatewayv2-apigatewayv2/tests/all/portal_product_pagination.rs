// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `ListPortalProducts`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `ListPortalProducts` reports the portal products the account holds, each one
//! > projected exactly as the single-product reader reports it, OLDEST FIRST — in the
//! > order the account created them — in a page of at most `MaxResults` items (and never
//! > more than the service's
//! > page maximum) chained through `NextToken`, so walking the chain yields every product
//! > exactly once whatever page size the caller asks for; `ResourceOwner` selects whose
//! > products are reported, so a request naming the owner that is not this account gets
//! > an empty page; a `MaxResults` that is not a positive integer, a `NextToken` this
//! > service did not issue for this collection and a `ResourceOwner` outside the owner
//! > catalog are each 400 `BadRequestException`; and the listing changes nothing.
//!
//! The groups a walk covers and the token that carries it from one to the next are
//! asserted in `portal_product_listing_order`.
//!
//! The rows vary what the rule generalises over: how many products the account holds (0
//! up to one past the page maximum), the page size (1 up to one past the total, plus the
//! page maximum and one past it), which optional members each product carries (a
//! description absent, empty, maximal or outside ASCII; no tags, one tag, several), and
//! which owner the request selects. The recorded unit that filed the issue — listing the
//! portal products an account just created — is one row.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    INVALID_NEXT_TOKEN_MESSAGE, MAX_PAGE_SIZE, OTHER_ACCOUNTS_RESOURCE_OWNER,
    RESOURCE_OWNER_VALUES,
};

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code, instant};

/// The portal-products collection URI (`ListPortalProducts`'s modelled
/// `GET /v2/portalproducts`).
const COLLECTION: &str = "/v2/portalproducts";

/// Create one portal product from `request` and answer its `PortalProductId`.
async fn create_product(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", COLLECTION, Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreatePortalProduct, got {status} {created}"
    );
    as_str(&created, "portalProductId")
}

/// Create one portal product carrying only the required member.
async fn create_plain_product(
    emulator: &MultiServiceRouter,
    case: &str,
    display_name: &str,
) -> String {
    create_product(emulator, case, &json!({ "displayName": display_name })).await
}

/// One page of `ListPortalProducts`, which must answer 200.
async fn list_products(emulator: &MultiServiceRouter, case: &str, query: &str) -> Value {
    let (status, body) = call(emulator, "GET", &format!("{COLLECTION}{query}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListPortalProducts, got {status} {body}"
    );
    body
}

/// The items one `ListPortalProducts` page reports.
fn items(body: &Value) -> Vec<Value> {
    body["items"].as_array().cloned().unwrap_or_default()
}

/// Walk `ListPortalProducts` from the first page to the last and answer every
/// `PortalProductId` seen, in the order the pages reported them.
async fn walk_pages(
    emulator: &MultiServiceRouter,
    label: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is caught
    // instead of looping forever.
    for _ in 0..total + 2 {
        let query = match &token {
            Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
            None => format!("?maxResults={page_size}"),
        };
        let body = list_products(emulator, label, &query).await;
        let page = items(&body);
        assert!(
            page.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(page.iter().map(|item| as_str(item, "portalProductId")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: expected the chain to end, got a NextToken {token:?} after {} items",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn list_portal_products_pages_every_product_exactly_once_at_every_page_size() {
    // The rule generalises over how many products exist and over page size, so the table
    // walks both: each count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5, 12] {
        let emulator = emulator();
        // In CREATION order, which is the order the chain must report them in: a minted
        // `PortalProductId` carries no ordering, so the ids below are in no particular
        // order.
        let mut created: Vec<String> = Vec::new();
        for index in 0..total {
            created.push(
                create_plain_product(
                    &emulator,
                    &format!("total={total}, product #{index}"),
                    &format!("Paged Product {index}"),
                )
                .await,
            );
        }
        let expected: BTreeSet<String> = created.iter().cloned().collect();

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let seen = walk_pages(&emulator, &label, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} products across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored products, got {seen:?}"
            );
            assert_eq!(
                seen, created,
                "case {label:?}: expected the chain oldest first, got {seen:?} against the \
                 created order {created:?}"
            );
        }

        // Unpaged: every product the account owns in one answer. The walk is not over —
        // the products other accounts shared with this one are the group still to come —
        // so the answer carries the token that starts it, and resuming there ends the
        // chain. An account holding none still reports an empty collection rather than no
        // collection.
        let body = list_products(&emulator, &format!("total={total}"), "").await;
        assert_eq!(
            items(&body).len(),
            total,
            "case total={total}: an unpaged ListPortalProducts must report every product, \
             got {body}"
        );
        let token = as_str(&body, "nextToken");
        assert!(
            !token.is_empty(),
            "case total={total}: expected the token that starts the shared group, got {body}"
        );
        let shared = list_products(
            &emulator,
            &format!("total={total}, the shared group"),
            &format!("?nextToken={token}"),
        )
        .await;
        assert!(
            items(&shared).is_empty() && carries_nothing(&shared, "nextToken"),
            "case total={total}: expected the shared group to end the chain, got {shared}"
        );
    }
}

#[tokio::test]
async fn list_portal_products_reports_the_record_get_portal_product_reports() {
    let emulator = emulator();

    // The rows differ in which optional members each product carries: a description
    // absent, empty, at the modelled maximum (`len 0..1024`) and outside ASCII; no tags,
    // one tag and several.
    let rows: Vec<(&'static str, Value)> = vec![
        (
            "only the required member",
            json!({ "displayName": "Listed Bare Product" }),
        ),
        (
            "a description and two tags",
            json!({
                "displayName": "Listed Payments API",
                "description": "everything a payments integrator needs",
                "tags": { "team": "payments", "tier": "gold" }
            }),
        ),
        (
            "an empty description and an empty tag collection",
            json!({ "displayName": "Listed Empty", "description": "", "tags": {} }),
        ),
        (
            "a description at the modelled maximum length",
            json!({ "displayName": "Listed Maximal", "description": "d".repeat(1024) }),
        ),
        (
            "a display name and description outside ASCII, with one tag",
            json!({
                "displayName": "Zahlungs-API 決済",
                "description": "決済 API — ünïcödé, emoji 🎉",
                "tags": { "owner": "api-team" }
            }),
        ),
    ];

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for (case, request) in &rows {
        let product_id = create_product(&emulator, case, request).await;
        expected.insert(product_id.clone());

        let body = list_products(&emulator, case, "").await;
        let listed = items(&body)
            .into_iter()
            .find(|item| as_str(item, "portalProductId") == product_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {product_id} to be listed, got {body}")
            });

        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("{COLLECTION}/{product_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetPortalProduct, got {status} {fetched}"
        );

        // The listed summary projects the stored record: every member it carries is the
        // value the single-product reader reports.
        for member in ["displayName", "portalProductArn", "portalProductId"] {
            assert_eq!(
                listed.get(member),
                fetched.get(member),
                "case {case:?}: expected ListPortalProducts to report the {member} \
                 GetPortalProduct reports, got {listed} and {fetched}"
            );
        }
        assert_eq!(
            instant(case, &listed, "lastModified"),
            instant(case, &fetched, "lastModified"),
            "case {case:?}: expected the stored LastModified, got {listed} and {fetched}"
        );
        assert_eq!(
            listed.get("tags"),
            fetched.get("tags"),
            "case {case:?}: expected the stored tags, got {listed} and {fetched}"
        );
        // `PortalProductSummary$Description` is a REQUIRED member of the summary while
        // `GetPortalProduct$Description` is optional, so a product created without a
        // description is summarised with no description text — never with text this
        // service invented.
        let described = request
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            listed.get("description"),
            Some(&json!(described)),
            "case {case:?}: expected the description the request carried, got {listed}"
        );
        // A summary carries no `DisplayOrder`: the member is not on the modelled shape.
        assert!(
            listed.get("displayOrder").is_none_or(Value::is_null),
            "case {case:?}: expected no displayOrder on a listed summary, got {listed}"
        );
    }

    // Every product the account holds is listed, and nothing else.
    let body = list_products(&emulator, "the whole account", "").await;
    let listed: BTreeSet<String> = items(&body)
        .iter()
        .map(|item| as_str(item, "portalProductId"))
        .collect();
    assert_eq!(
        listed, expected,
        "case \"the whole account\": expected exactly the stored products, got {body}"
    );
}

#[tokio::test]
async fn list_portal_products_reports_the_products_the_selected_owner_owns() {
    let emulator = emulator();
    let mut stored: BTreeSet<String> = BTreeSet::new();
    for index in 0..4 {
        stored.insert(
            create_plain_product(
                &emulator,
                &format!("product #{index}"),
                &format!("Owned Product {index}"),
            )
            .await,
        );
    }

    // Every owner the member selects between, plus the request that names none. This
    // emulator serves one account, which owns every product it holds, so the owner that
    // is not this account owns none of them.
    let rows: Vec<(&str, String, usize)> = vec![
        ("no owner at all", String::new(), stored.len()),
        (
            "this account's own products",
            "?resourceOwner=SELF".to_string(),
            stored.len(),
        ),
        (
            "the products owned by other accounts",
            format!("?resourceOwner={OTHER_ACCOUNTS_RESOURCE_OWNER}"),
            0,
        ),
    ];
    for (case, query, expected_count) in rows {
        let body = list_products(&emulator, case, &query).await;
        let page = items(&body);
        assert_eq!(
            page.len(),
            expected_count,
            "case {case:?}: expected {expected_count} products, got {body}"
        );
        if expected_count == 0 {
            // A page of nothing has nothing to resume.
            assert!(
                carries_nothing(&body, "nextToken"),
                "case {case:?}: expected no NextToken over an empty collection, got {body}"
            );
        } else {
            assert_eq!(
                page.iter()
                    .map(|item| as_str(item, "portalProductId"))
                    .collect::<BTreeSet<_>>(),
                stored,
                "case {case:?}: expected exactly the stored products, got {body}"
            );
        }
    }

    // A page size below the total does not let the owner that owns nothing report
    // something: the filter is over the owner, not over the page.
    let body = list_products(
        &emulator,
        "a paged request for another owner's products",
        &format!("?maxResults=1&resourceOwner={OTHER_ACCOUNTS_RESOURCE_OWNER}"),
    )
    .await;
    assert!(
        items(&body).is_empty() && carries_nothing(&body, "nextToken"),
        "case \"a paged request for another owner's products\": expected an empty page, \
         got {body}"
    );

    // A value outside the owner catalog names no owner, so the request is not valid.
    for raw in ["self", "OTHER-ACCOUNTS", "000000000000", "", "ALL", "OTHER_ACCOUNTS%20"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{COLLECTION}?resourceOwner={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case resourceOwner={raw:?}: expected 400 from ListPortalProducts, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case resourceOwner={raw:?}: expected BadRequestException, got {body}"
        );
        let message = as_str(&body, "message");
        assert!(
            message.contains("ResourceOwner")
                && RESOURCE_OWNER_VALUES
                    .iter()
                    .all(|owner| message.contains(owner)),
            "case resourceOwner={raw:?}: expected a refusal naming the member and the \
             owners it accepts, got {message:?}"
        );
    }
}

#[tokio::test]
async fn list_portal_products_is_a_report_that_changes_nothing() {
    let emulator = emulator();
    for index in 0..3 {
        create_plain_product(
            &emulator,
            &format!("product #{index}"),
            &format!("Repeated Product {index}"),
        )
        .await;
    }

    let first = list_products(&emulator, "the first listing", "?maxResults=2").await;
    for attempt in 0..3 {
        let again =
            list_products(&emulator, &format!("attempt={attempt}"), "?maxResults=2").await;
        assert_eq!(
            again, first,
            "case attempt={attempt}: a report must not change what a later caller sees, \
             got {again}"
        );
    }

    // A refused listing changes nothing either, and the products are still all there.
    let (_, _) = call(
        &emulator,
        "GET",
        &format!("{COLLECTION}?resourceOwner=nobody"),
        None,
    )
    .await;
    let after = list_products(&emulator, "after a refusal", "?maxResults=2").await;
    assert_eq!(
        after, first,
        "case \"after a refusal\": expected the same page, got {after}"
    );
}

#[tokio::test]
async fn list_portal_products_refuses_a_max_results_that_is_not_a_positive_integer() {
    let emulator = emulator();
    create_plain_product(&emulator, "a refusals host", "Refusals Product").await;

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's: a
    // positive decimal integer. Each row breaks it in a different way.
    for raw in ["0", "-1", "abc", "1.5", "%20", "1e3"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{COLLECTION}?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from ListPortalProducts, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // The lower bound of the accepted range, the page maximum and one past it.
    for raw in ["1", "100", "101", "2147483647"] {
        list_products(
            &emulator,
            &format!("maxResults={raw}"),
            &format!("?maxResults={raw}"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_list_portal_products_page_is_bounded_server_side() {
    let emulator = emulator();
    // One product past the page maximum, so a request asking for more than the maximum
    // has more to report than a page may carry.
    let total = MAX_PAGE_SIZE + 1;
    for index in 0..total {
        create_plain_product(
            &emulator,
            &format!("product #{index}"),
            &format!("Clamped Product {index}"),
        )
        .await;
    }

    for (label, query) in [
        ("no MaxResults", String::new()),
        (
            "MaxResults far above the maximum",
            "?maxResults=1000000".to_string(),
        ),
        (
            "MaxResults at the maximum",
            format!("?maxResults={MAX_PAGE_SIZE}"),
        ),
        (
            "MaxResults one past the maximum",
            format!("?maxResults={}", MAX_PAGE_SIZE + 1),
        ),
    ] {
        let body = list_products(&emulator, label, &query).await;
        let page = items(&body);
        assert_eq!(
            page.len(),
            MAX_PAGE_SIZE,
            "case {label:?}: expected a page clamped to the service maximum, got {}",
            page.len()
        );
        // More remains than one page can carry, so the walk is resumable.
        assert!(
            !carries_nothing(&body, "nextToken"),
            "case {label:?}: expected a NextToken while products remain, got {body}"
        );
    }

    // The chain still reports every product exactly once.
    let seen = walk_pages(&emulator, "the whole clamped chain", MAX_PAGE_SIZE, total).await;
    assert_eq!(
        seen.len(),
        total,
        "case \"the whole clamped chain\": expected {total} products, got {}",
        seen.len()
    );
}

#[tokio::test]
async fn list_portal_products_refuses_a_next_token_it_did_not_issue() {
    let emulator = emulator();
    let mut minted: Vec<String> = Vec::new();
    for index in 0..3 {
        minted.push(
            create_plain_product(
                &emulator,
                &format!("product #{index}"),
                &format!("Token Product {index}"),
            )
            .await,
        );
    }
    // Two apis, so a sibling collection of the same account issues a token of its own.
    for name in ["product-token-api-a", "product-token-api-b"] {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({ "name": name, "protocolType": "HTTP" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected 201 from CreateApi, got {status} {created}"
        );
    }

    // A token this service issued for the account's portal products resumes the walk.
    let first = list_products(&emulator, "the first page", "?maxResults=1").await;
    let issued = as_str(&first, "nextToken");
    assert!(
        !issued.is_empty(),
        "case \"the first page\": expected a NextToken, got {first}"
    );
    let resumed = list_products(
        &emulator,
        "the issued token",
        &format!("?maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        items(&resumed).len(),
        1,
        "case \"the issued token\": expected the next page, got {resumed}"
    );

    let (status, api_page) = call(&emulator, "GET", "/v2/apis?maxResults=1", None).await;
    assert_eq!(
        status, 200,
        "case \"the apis listing\": expected 200 from GetApis, got {status} {api_page}"
    );
    let apis_token = as_str(&api_page, "nextToken");
    // Two apis at a page size of one, so the sibling collection certainly issued a token:
    // asserting it here keeps the cross-collection row below unconditional.
    assert!(
        !apis_token.is_empty(),
        "case \"the apis listing\": expected a NextToken from GetApis, got {api_page}"
    );

    // The rows vary over what a token the service did not issue can look like: a word, a
    // product id (the shape of a key), hex of nothing this service minted, an empty
    // value, a truncated issued token, one far longer than any minted token, and a token
    // issued for a SIBLING collection of the same account.
    let cases: Vec<(String, String)> = vec![
        ("a word".to_string(), "not-a-token".to_string()),
        ("a product id".to_string(), minted[0].clone()),
        (
            "hex of nothing this service minted".to_string(),
            "zzzz".to_string(),
        ),
        ("an empty token".to_string(), String::new()),
        (
            "an issued token with its last character dropped".to_string(),
            issued[..issued.len() - 1].to_string(),
        ),
        (
            "a token far longer than any this service mints".to_string(),
            "ab".repeat(4096),
        ),
        (
            "a token issued for the account's apis".to_string(),
            apis_token,
        ),
    ];

    for (case, token) in &cases {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("{COLLECTION}?nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from ListPortalProducts for token {token:?}, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_NEXT_TOKEN_MESSAGE,
            "case {case:?}: expected the message {INVALID_NEXT_TOKEN_MESSAGE:?}, got {body}"
        );
    }
}
