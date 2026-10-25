// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `ProductPage` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateProductPage` persists a product page UNDER the portal product the URI
//!   names and answers 201** with the `ProductPageId` it minted (ten lowercase
//!   alphanumerics, which satisfies the modelled `__stringMin10Max30PatternAZ09`), the
//!   ARN naming that product, that page and the Region the request addressed, a
//!   `LastModified` reading the write instant, and the `DisplayContent` the request
//!   carried. `GetProductPage` reports that same record, member for member, and reading
//!   it changes nothing a later caller sees.
//! * **A page is addressed under its own product**: a product nothing minted is 404, and
//!   so is a page id nothing minted, a page read under another product, and a read naming
//!   another account as the resource owner.
//! * **Each minted `ProductPageId` is distinct within its product**, and two products
//!   hold their own pages.
//! * **A request that does not satisfy the modelled `DisplayContent` — either member
//!   absent, empty, or past its published length — is refused 400
//!   `BadRequestException`** and persists nothing.
//!
//! The rows vary what each rule generalises over: the title and body content (shortest
//! accepted, multi-line, non-ASCII, and both members at their modelled maxima), the
//! number of pages held under one product, and the Region the request is signed for. The
//! recorded lifecycle that filed the issue — one create of one page, read back — is ONE
//! row of the first table.

use serde_json::{json, Value};

use crate::api_lifecycle::{
    as_str, assert_minted_id, call, call_in_region, emulator, error_code, instant,
};
use crate::product_rest_endpoint_page_lifecycle::{product, PRODUCTS, REGION};

/// How far the reported `LastModified` may sit from this test's clock reading, in
/// seconds. The service reports the instant at second granularity, so a whole second of
/// truncation plus scheduling slack is allowed; the point of the assertion is that the
/// member reads the write instant rather than zero or a fixed literal.
const LAST_MODIFIED_SLACK_SECONDS: f64 = 10.0;

/// The published maximum of `DisplayContent$Title`
/// (`smithy_query(query="shape", shape="DisplayContent")`: `Title` targets
/// `__stringMin1Max255`).
const TITLE_MAX_LENGTH: usize = 255;

/// The published maximum of `DisplayContent$Body` (the same member list: `Body` targets
/// `__stringMin1Max32768`).
const BODY_MAX_LENGTH: usize = 32_768;

/// The product-pages collection of one product (`POST|GET
/// /v2/portalproducts/{PortalProductId}/productpages`).
fn pages_of(portal_product_id: &str) -> String {
    format!("{PRODUCTS}/{portal_product_id}/productpages")
}

/// The ARN API Gateway names a product page by: the page's own REST path under its
/// product, in the Region the creating request addressed. Written out here rather than
/// taken from the service, so the table asserts the rule rather than the implementation.
fn expected_arn(region: &str, portal_product_id: &str, product_page_id: &str) -> String {
    format!(
        "arn:aws:apigateway:{region}:{}:/portalproducts/{portal_product_id}\
         /productpages/{product_page_id}",
        ::engine::emulator_account_id()
    )
}

/// One accepted-create row: the `DisplayContent` the request carries.
struct Row {
    case: &'static str,
    title: String,
    body: String,
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a plain title and body",
            title: "Getting started".to_string(),
            body: "Call the orders endpoint to place an order.".to_string(),
        },
        Row {
            case: "both members at the shortest length the model accepts (1)",
            title: "t".to_string(),
            body: "b".to_string(),
        },
        Row {
            case: "a multi-line markdown body",
            title: "Reference".to_string(),
            body: "# Orders\n\n- `GET /orders`\n- `POST /orders`\n\nSee the guide.\n"
                .to_string(),
        },
        Row {
            case: "non-ASCII title and body",
            title: "Übersicht — 概要".to_string(),
            body: "Grüße! 価格は無料です。\u{1f680}".to_string(),
        },
        Row {
            case: "both members at their modelled maxima",
            title: "t".repeat(TITLE_MAX_LENGTH),
            body: "b".repeat(BODY_MAX_LENGTH),
        },
    ]
}

/// The create's response, and the read-back, must report the row's content.
fn assert_reports_row(case: &str, row: &Row, body: &Value) {
    let content = body
        .get("displayContent")
        .unwrap_or_else(|| panic!("case {case:?}: the response must report a displayContent: {body}"));
    assert_eq!(
        content.get("title").and_then(Value::as_str),
        Some(row.title.as_str()),
        "case {case:?}: expected the stored title, got {:?}",
        content.get("title")
    );
    assert_eq!(
        content.get("body").and_then(Value::as_str),
        Some(row.body.as_str()),
        "case {case:?}: expected the stored body, got a body of {:?} characters",
        content.get("body").and_then(Value::as_str).map(str::len)
    );
}

#[tokio::test]
async fn create_product_page_persists_the_record_get_product_page_reports_back() {
    let emulator = emulator();
    let product_id = product(&emulator, "pages-under-one-product").await;
    let collection = pages_of(&product_id);

    for row in accepted_rows() {
        let case = row.case;
        let request = json!({ "displayContent": { "title": row.title, "body": row.body } });
        let (status, created) = call(&emulator, "POST", &collection, Some(request)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
        );

        let page_id = as_str(&created, "productPageId");
        assert_minted_id(case, "productPageId", &page_id);
        assert_eq!(
            as_str(&created, "productPageArn"),
            expected_arn(REGION, &product_id, &page_id),
            "case {case:?}: expected the page's own ARN, got {:?}",
            as_str(&created, "productPageArn")
        );
        assert_reports_row(case, &row, &created);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default();
        let last_modified = instant(case, &created, "lastModified");
        assert!(
            (now - last_modified).abs() <= LAST_MODIFIED_SLACK_SECONDS,
            "case {case:?}: LastModified must read the write instant; got {last_modified} \
             against a clock reading {now}"
        );

        // Read it back twice: the Get reports the stored record, and reporting it leaves
        // the store exactly as the next caller finds it.
        let page_uri = format!("{collection}/{page_id}");
        let (read_status, read) = call(&emulator, "GET", &page_uri, None).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetProductPage, got {read_status} {read}"
        );
        assert_eq!(
            read, created,
            "case {case:?}: GetProductPage must report the created page member for member"
        );
        let (again_status, again) = call(&emulator, "GET", &page_uri, None).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected 200 from a second GetProductPage, got {again_status} {again}"
        );
        assert_eq!(
            again, read,
            "case {case:?}: reading a page must not change what a later caller sees"
        );
    }
}

#[tokio::test]
async fn each_product_holds_its_own_pages_with_distinct_ids() {
    let emulator = emulator();
    let first = product(&emulator, "first-product").await;
    let second = product(&emulator, "second-product").await;

    // Three pages under the first product, one under the second: every id distinct within
    // its product, and the second product's page is not reachable under the first.
    let mut first_ids = Vec::new();
    for index in 0..3u32 {
        let case = format!("page {index} of the first product");
        let (status, created) = call(
            &emulator,
            "POST",
            &pages_of(&first),
            Some(json!({
                "displayContent": { "title": format!("Page {index}"), "body": format!("Body {index}") }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
        );
        let id = as_str(&created, "productPageId");
        assert_minted_id(&case, "productPageId", &id);
        assert!(
            !first_ids.contains(&id),
            "case {case:?}: expected a fresh productPageId, got the already-minted {id:?}"
        );
        first_ids.push(id);
    }

    let (status, other) = call(
        &emulator,
        "POST",
        &pages_of(&second),
        Some(json!({ "displayContent": { "title": "Elsewhere", "body": "Another product" } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductPage under the second product, got {status} {other}"
    );
    let other_id = as_str(&other, "productPageId");
    assert_eq!(
        as_str(&other, "productPageArn"),
        expected_arn(REGION, &second, &other_id),
        "the ARN must name the product the page was created under"
    );

    // Each of the first product's pages still reads back under the first product.
    for id in &first_ids {
        let (read_status, read) =
            call(&emulator, "GET", &format!("{}/{id}", pages_of(&first)), None).await;
        assert_eq!(
            read_status, 200,
            "case {id:?}: expected 200 from GetProductPage, got {read_status} {read}"
        );
    }

    // A page id is only unique WITHIN the product that owns it, so each product's first
    // page carries the same id: what the FIRST product reports for that id is its OWN
    // page, never the second product's.
    assert_eq!(
        other_id,
        first_ids[0],
        "a ProductPageId is minted per product, so each product's first page carries the \
         same id; got {other_id:?} against the first product's {:?}",
        first_ids[0]
    );
    let (shared_status, shared) =
        call(&emulator, "GET", &format!("{}/{other_id}", pages_of(&first)), None).await;
    assert_eq!(
        shared_status, 200,
        "expected 200 reading the first product's own page {other_id:?}, got {shared_status} {shared}"
    );
    assert_eq!(
        as_str(&shared, "productPageArn"),
        expected_arn(REGION, &first, &other_id),
        "a page read under the first product must be the first product's own, got {:?}",
        as_str(&shared, "productPageArn")
    );
    assert_eq!(
        shared
            .get("displayContent")
            .and_then(|content| content.get("title"))
            .and_then(Value::as_str),
        Some("Page 0"),
        "the first product must report its own page, not the second product's: {shared}"
    );

    // An id only the SECOND product minted names no page of the first: the fourth page of
    // the second product is past the three the first product holds.
    let mut fourth_id = String::new();
    for index in 1..4u32 {
        let (status, created) = call(
            &emulator,
            "POST",
            &pages_of(&second),
            Some(json!({
                "displayContent": { "title": format!("Elsewhere {index}"), "body": "Another product" }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "expected 201 from CreateProductPage under the second product, got {status} {created}"
        );
        fourth_id = as_str(&created, "productPageId");
    }
    assert!(
        !first_ids.contains(&fourth_id),
        "the second product's fourth page id {fourth_id:?} must be one the first product \
         did not mint, got the first product's {first_ids:?}"
    );
    let (cross_status, cross) = call(
        &emulator,
        "GET",
        &format!("{}/{fourth_id}", pages_of(&first)),
        None,
    )
    .await;
    assert_eq!(
        cross_status, 404,
        "expected 404 reading the second product's page under the first, got {cross_status} {cross}"
    );
}

#[tokio::test]
async fn a_product_page_arn_names_the_region_the_request_addressed() {
    let emulator = emulator();

    // The ARN of a resource names the Region it was created in, so a create signed for
    // another Region reports that Region — and the page still reads back there.
    for region in ["us-east-1", "ap-southeast-2", REGION] {
        let (status, created_product) = call_in_region(
            &emulator,
            region,
            "POST",
            PRODUCTS,
            Some(json!({ "displayName": format!("product-in-{region}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region:?}: expected 201 from CreatePortalProduct, got {status} {created_product}"
        );
        let product_id = as_str(&created_product, "portalProductId");

        let (status, created) = call_in_region(
            &emulator,
            region,
            "POST",
            &pages_of(&product_id),
            Some(json!({ "displayContent": { "title": "Regional", "body": "Regional body" } })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {region:?}: expected 201 from CreateProductPage, got {status} {created}"
        );
        let page_id = as_str(&created, "productPageId");
        assert_eq!(
            as_str(&created, "productPageArn"),
            expected_arn(region, &product_id, &page_id),
            "case {region:?}: expected an ARN naming that Region, got {:?}",
            as_str(&created, "productPageArn")
        );

        // The ARN is the one the create stored, so a read signed for a DIFFERENT Region
        // reports the same ARN rather than rebuilding it from the reader's Region.
        let (read_status, read) = call_in_region(
            &emulator,
            "eu-central-1",
            "GET",
            &format!("{}/{page_id}", pages_of(&product_id)),
            None,
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {region:?}: expected 200 from GetProductPage, got {read_status} {read}"
        );
        assert_eq!(
            as_str(&read, "productPageArn"),
            expected_arn(region, &product_id, &page_id),
            "case {region:?}: the stored ARN must not follow the reader's Region"
        );
    }
}

#[tokio::test]
async fn a_product_page_nothing_minted_is_not_found() {
    let emulator = emulator();
    let product_id = product(&emulator, "not-found-cases").await;
    let (status, created) = call(
        &emulator,
        "POST",
        &pages_of(&product_id),
        Some(json!({ "displayContent": { "title": "Only page", "body": "Only body" } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productPageId");

    // Each row names a page the emulator does not hold: through an unknown product, an
    // unknown page id (short and at the 30-character modelled maximum), and a read whose
    // resource owner is another account.
    let rows: Vec<(String, String)> = vec![
        (
            "an unknown product".to_string(),
            format!("{}/{page_id}", pages_of("zzzzzzzzzz")),
        ),
        (
            "an unknown page under a known product".to_string(),
            format!("{}/zzzzzzzzzz", pages_of(&product_id)),
        ),
        (
            "a long page id nothing minted".to_string(),
            format!("{}/{}", pages_of(&product_id), "z".repeat(30)),
        ),
        (
            "a known page owned by another account".to_string(),
            format!(
                "{}/{page_id}?resourceOwnerAccountId=111122223333",
                pages_of(&product_id)
            ),
        ),
    ];

    for (case, uri) in rows {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetProductPage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // Creating a page under a product nothing minted is the same refusal.
    let (status, body) = call(
        &emulator,
        "POST",
        &pages_of("zzzzzzzzzz"),
        Some(json!({ "displayContent": { "title": "Orphan", "body": "Orphan body" } })),
    )
    .await;
    assert_eq!(
        status, 404,
        "expected 404 from CreateProductPage under an unknown product, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "expected NotFoundException from CreateProductPage under an unknown product, got {body}"
    );

    // The emulator's own account IS the resource owner, so naming it reads the page.
    let (status, read) = call(
        &emulator,
        "GET",
        &format!(
            "{}/{page_id}?resourceOwnerAccountId={}",
            pages_of(&product_id),
            ::engine::emulator_account_id()
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 reading the page as its own account's resource, got {status} {read}"
    );
}

#[tokio::test]
async fn a_display_content_the_model_does_not_allow_is_refused() {
    let emulator = emulator();
    let product_id = product(&emulator, "refused-content").await;
    let collection = pages_of(&product_id);

    // Every row fails a rule the model declares for `DisplayContent`: a required member
    // absent, a member below its minimum length (1), or one past its published maximum.
    let rows: Vec<(&str, Value)> = vec![
        ("no DisplayContent at all", json!({})),
        (
            "no Title",
            json!({ "displayContent": { "body": "a body with no title" } }),
        ),
        (
            "no Body",
            json!({ "displayContent": { "title": "a title with no body" } }),
        ),
        (
            "an empty Title",
            json!({ "displayContent": { "title": "", "body": "a body" } }),
        ),
        (
            "an empty Body",
            json!({ "displayContent": { "title": "a title", "body": "" } }),
        ),
        (
            "a Title one character past the maximum",
            json!({
                "displayContent": {
                    "title": "t".repeat(TITLE_MAX_LENGTH + 1),
                    "body": "a body"
                }
            }),
        ),
        (
            "a Body one character past the maximum",
            json!({
                "displayContent": {
                    "title": "a title",
                    "body": "b".repeat(BODY_MAX_LENGTH + 1)
                }
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(&emulator, "POST", &collection, Some(request)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateProductPage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // A refused create persisted nothing, so the first page this product holds is the one
    // the next accepted create mints.
    let (status, created) = call(
        &emulator,
        "POST",
        &collection,
        Some(json!({ "displayContent": { "title": "Accepted", "body": "Accepted body" } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductPage after the refusals, got {status} {created}"
    );
    let page_id = as_str(&created, "productPageId");
    let (read_status, read) = call(
        &emulator,
        "GET",
        &format!("{collection}/{page_id}"),
        None,
    )
    .await;
    assert_eq!(
        read_status, 200,
        "expected 200 from GetProductPage, got {read_status} {read}"
    );
    assert_eq!(
        read, created,
        "GetProductPage must report the page the accepted create stored"
    );
}
