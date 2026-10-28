// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `UpdateProductPage`.
//!
//! The rules these cases assert:
//!
//! * **The update is a PATCH**
//!   (`PATCH /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}`): a member
//!   the request carries replaces the stored one, a member it omits leaves the stored value
//!   exactly as it was, and the members this operation does not take (the page's id and its
//!   ARN) are carried over untouched. What the response reports is what a later
//!   `GetProductPage` reports, so the patch is persisted rather than echoed.
//! * **A `DisplayContent` the patch carries replaces the page's content wholesale**:
//!   `Title` and `Body` are both required on the modelled structure
//!   (`smithy_query(query="shape", shape="DisplayContent")`), so a content the request
//!   carries fixes both members.
//! * **`LastModified` advances when the patch carries a member to apply**, and a patch
//!   carrying none leaves the record — `LastModified` included — exactly as it was.
//! * **A page is patched only through its own product and its own minted id**: an unknown
//!   product, an unknown page id and a page addressed under another product are each 404
//!   `NotFoundException` with the sentence `GetProductPage` refuses the same address with,
//!   and write nothing.
//! * **A patch whose content the model does not allow is refused 400 and persists
//!   nothing**: either member absent, empty, or one character past its published maximum.
//!
//! The rows vary what the rules generalise over: the content's length (the modelled
//! minimum of 1, a plain sentence, the modelled maxima), its script and line structure,
//! whether the patch repeats the stored value, how many patches land on one page, and the
//! Region the patching caller signs for. The recorded lifecycle that filed the issue —
//! create, patch, read back — is ONE row of the first table.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, call_in_region, emulator, error_code, instant};
use crate::product_page_lifecycle::{pages_of, BODY_MAX_LENGTH, TITLE_MAX_LENGTH};
use crate::product_rest_endpoint_page_lifecycle::{content_member, product, PRODUCTS, REGION};

/// How long to wait so a patch lands in a LATER second than the write before it. The
/// service reports `LastModified` at whole-second granularity, so only a wait past a
/// second boundary makes the move observable on the wire. A BLOCKING wait on purpose, for
/// the reason given in `deployment_stage_release`: this workspace's `tokio` carries no
/// `time` feature.
const SECOND_BOUNDARY_WAIT: Duration = Duration::from_millis(1_100);

// The published maxima of `DisplayContent$Title` (255) and `DisplayContent$Body` (32768)
// come from `TITLE_MAX_LENGTH` and `BODY_MAX_LENGTH`, the collection URI from `pages_of`
// and the reading of one `displayContent` member from `content_member`, all imported
// above, so the product-page suites cannot drift apart on any of them.

/// One patch row: the content the page is created with, the patch under test, and the
/// content the patched page must report.
struct Row {
    case: &'static str,
    created_title: String,
    created_body: String,
    /// The patch under test, as the client sends it.
    patch: Value,
    /// The `displayContent.title` the patched page must report.
    expected_title: String,
    /// The `displayContent.body` the patched page must report.
    expected_body: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a plain content replacing the created one",
            created_title: "Getting started".to_string(),
            created_body: "Call the orders endpoint.".to_string(),
            patch: json!({
                "displayContent": { "title": "Reference", "body": "Every order operation." }
            }),
            expected_title: "Reference".to_string(),
            expected_body: "Every order operation.".to_string(),
        },
        Row {
            case: "both members at the shortest length the model accepts (1)",
            created_title: "Getting started".to_string(),
            created_body: "Call the orders endpoint.".to_string(),
            patch: json!({ "displayContent": { "title": "t", "body": "b" } }),
            expected_title: "t".to_string(),
            expected_body: "b".to_string(),
        },
        Row {
            case: "a multi-line markdown body",
            created_title: "t".to_string(),
            created_body: "b".to_string(),
            patch: json!({
                "displayContent": {
                    "title": "Reference",
                    "body": "# Orders\n\n- `GET /orders`\n- `POST /orders`\n\nSee the guide.\n"
                }
            }),
            expected_title: "Reference".to_string(),
            expected_body: "# Orders\n\n- `GET /orders`\n- `POST /orders`\n\nSee the guide.\n"
                .to_string(),
        },
        Row {
            case: "content outside ASCII, which round-trips unchanged",
            created_title: "Overview".to_string(),
            created_body: "An English body.".to_string(),
            patch: json!({
                "displayContent": { "title": "Übersicht — 概要", "body": "Grüße! 価格は無料です。\u{1f680}" }
            }),
            expected_title: "Übersicht — 概要".to_string(),
            expected_body: "Grüße! 価格は無料です。\u{1f680}".to_string(),
        },
        Row {
            case: "both members at their modelled maxima",
            created_title: "Short".to_string(),
            created_body: "Short body".to_string(),
            patch: json!({
                "displayContent": {
                    "title": "t".repeat(TITLE_MAX_LENGTH),
                    "body": "b".repeat(BODY_MAX_LENGTH)
                }
            }),
            expected_title: "t".repeat(TITLE_MAX_LENGTH),
            expected_body: "b".repeat(BODY_MAX_LENGTH),
        },
        Row {
            case: "a patch from the maxima back to the minimum length",
            created_title: "t".repeat(TITLE_MAX_LENGTH),
            created_body: "b".repeat(BODY_MAX_LENGTH),
            patch: json!({ "displayContent": { "title": "t", "body": "b" } }),
            expected_title: "t".to_string(),
            expected_body: "b".to_string(),
        },
        Row {
            case: "the content the page already carries, repeated",
            created_title: "Unchanged".to_string(),
            created_body: "Unchanged body".to_string(),
            patch: json!({
                "displayContent": { "title": "Unchanged", "body": "Unchanged body" }
            }),
            expected_title: "Unchanged".to_string(),
            expected_body: "Unchanged body".to_string(),
        },
        Row {
            case: "a patch carrying no member at all, which leaves the stored content",
            created_title: "Kept".to_string(),
            created_body: "Kept body".to_string(),
            patch: json!({}),
            expected_title: "Kept".to_string(),
            expected_body: "Kept body".to_string(),
        },
    ]
}

/// Read one page back through `GetProductPage`, asserting the read succeeded.
async fn get_page(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    uri: &str,
) -> Value {
    let (status, fetched) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetProductPage, got {status} {fetched}"
    );
    fetched
}

/// Create one product and one page under it, and answer `(page uri, the created page)`.
async fn page_under_a_fresh_product(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    title: &str,
    body: &str,
) -> (String, Value) {
    let product_id = product(emulator, case).await;
    let collection = pages_of(&product_id);
    let (status, created) = call(
        emulator,
        "POST",
        &collection,
        Some(json!({ "displayContent": { "title": title, "body": body } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
    );
    let page_id = as_str(&created, "productPageId");
    (format!("{collection}/{page_id}"), created)
}

/// The members of a page that no patch of this operation can move, so two readings of one
/// page are compared through ONE projection.
fn identity(document: &Value) -> Value {
    json!({
        "productPageId": document.get("productPageId"),
        "productPageArn": document.get("productPageArn")
    })
}

/// The member of a page a patch CAN move, beside its modification instant.
fn patchable(document: &Value) -> Value {
    json!({ "displayContent": document.get("displayContent") })
}

#[tokio::test]
async fn a_patch_replaces_the_content_it_carries_and_leaves_the_rest_as_it_was() {
    let emulator = emulator();

    for row in rows() {
        let (uri, created) =
            page_under_a_fresh_product(&emulator, row.case, &row.created_title, &row.created_body)
                .await;
        let before = get_page(&emulator, row.case, &uri).await;

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(row.patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateProductPage, got {status} {patched}",
            row.case
        );

        assert_eq!(
            content_member(&patched, "title"),
            Some(row.expected_title.clone()),
            "case {:?}: expected the patched page to report the title {:?}, got {:?}",
            row.case,
            row.expected_title,
            content_member(&patched, "title")
        );
        assert_eq!(
            content_member(&patched, "body"),
            Some(row.expected_body.clone()),
            "case {:?}: expected the patched page to report a body of {} characters, got {:?}",
            row.case,
            row.expected_body.len(),
            content_member(&patched, "body").map(|body| body.len())
        );

        // The members this operation does not take are carried over untouched.
        assert_eq!(
            identity(&patched),
            identity(&created),
            "case {:?}: expected the page's id and ARN to survive the patch, got {patched} \
             against {created}",
            row.case
        );

        // What the response reports is what a later reader reports: the patch was
        // persisted, not echoed.
        let fetched = get_page(&emulator, row.case, &uri).await;
        assert_eq!(
            patchable(&fetched),
            patchable(&patched),
            "case {:?}: expected GetProductPage to report the patched content, got {fetched} \
             against {patched}",
            row.case
        );
        assert_eq!(
            identity(&fetched),
            identity(&patched),
            "case {:?}: expected the stored id and ARN, got {fetched} against {patched}",
            row.case
        );
        assert_eq!(
            instant(row.case, &fetched, "lastModified"),
            instant(row.case, &patched, "lastModified"),
            "case {:?}: expected the stored LastModified, got {fetched} against {patched}",
            row.case
        );

        // `LastModified` reads this patch's own clock rather than a fixed literal, and
        // never sits before the instant the page was created at.
        assert!(
            instant(row.case, &patched, "lastModified")
                >= instant(row.case, &before, "lastModified"),
            "case {:?}: expected LastModified not to move backwards, got {patched} against \
             {before}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_patch_that_carries_the_content_advances_last_modified() {
    let emulator = emulator();

    // Two patches in a row, so the instant follows EACH write rather than only the first
    // one after the create.
    let patches = [
        (
            "the first patch after the create",
            json!({ "displayContent": { "title": "First", "body": "First body" } }),
        ),
        (
            "a second patch onto the patched page",
            json!({ "displayContent": { "title": "Second", "body": "Second body" } }),
        ),
    ];

    let (uri, created) = page_under_a_fresh_product(
        &emulator,
        "the clock of a patched page",
        "Created",
        "Created body",
    )
    .await;
    let mut previous = instant("the create", &created, "lastModified");

    for (case, patch) in patches {
        sleep(SECOND_BOUNDARY_WAIT);

        let (status, patched) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductPage, got {status} {patched}"
        );
        let patched_at = instant(case, &patched, "lastModified");
        assert!(
            patched_at > previous,
            "case {case:?}: expected LastModified to advance past the previous write instant \
             {previous}, got {patched_at}"
        );

        let fetched = get_page(&emulator, case, &uri).await;
        assert_eq!(
            instant(case, &fetched, "lastModified"),
            patched_at,
            "case {case:?}: expected GetProductPage to report the patched LastModified, got \
             {fetched}"
        );
        previous = patched_at;
    }
}

#[tokio::test]
async fn a_patch_that_carries_no_member_leaves_the_record_exactly_as_it_was() {
    let emulator = emulator();
    let case = "a patch carrying none of the page's members";

    let (uri, _created) = page_under_a_fresh_product(
        &emulator,
        case,
        "Nothing in the patch applies",
        "The stored body",
    )
    .await;

    // Past a second boundary, so a `LastModified` that moved would be visible at the
    // whole-second granularity the member is reported at.
    sleep(SECOND_BOUNDARY_WAIT);
    let before = get_page(&emulator, case, &uri).await;

    let (status, patched) = call(&emulator, "PATCH", &uri, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateProductPage, got {status} {patched}"
    );
    assert_eq!(
        patchable(&patched),
        patchable(&before),
        "case {case:?}: expected the content to be reported unchanged, got {patched} against \
         {before}"
    );
    assert_eq!(
        instant(case, &patched, "lastModified"),
        instant(case, &before, "lastModified"),
        "case {case:?}: expected LastModified to stand still, got {patched} against {before}"
    );

    let after = get_page(&emulator, case, &uri).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the stored record to be unchanged, got {after} against {before}"
    );
}

#[tokio::test]
async fn a_patch_signed_for_another_region_leaves_the_pages_own_arn_alone() {
    let emulator = emulator();

    // An ARN names the Region its resource was created in, so a patch signed elsewhere
    // reports the ARN the create stored rather than rebuilding it from the patching
    // caller's Region.
    for (home, patched_from) in [
        ("ap-southeast-2", "us-east-1"),
        ("us-east-1", "eu-central-1"),
        (REGION, "ap-southeast-2"),
    ] {
        let case = home;
        let (status, created_product) = call_in_region(
            &emulator,
            home,
            "POST",
            PRODUCTS,
            Some(json!({ "displayName": format!("product in {home}") })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreatePortalProduct, got {status} {created_product}"
        );
        let product_id = as_str(&created_product, "portalProductId");
        let collection = pages_of(&product_id);

        let (status, created) = call_in_region(
            &emulator,
            home,
            "POST",
            &collection,
            Some(json!({ "displayContent": { "title": "Regional", "body": "Regional body" } })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateProductPage, got {status} {created}"
        );
        let page_id = as_str(&created, "productPageId");
        let uri = format!("{collection}/{page_id}");

        let (status, patched) = call_in_region(
            &emulator,
            patched_from,
            "PATCH",
            &uri,
            Some(json!({ "displayContent": { "title": "Patched", "body": "Patched body" } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateProductPage signed for {patched_from:?}, got \
             {status} {patched}"
        );
        assert_eq!(
            as_str(&patched, "productPageArn"),
            as_str(&created, "productPageArn"),
            "case {case:?}: expected the stored ARN to survive a patch signed elsewhere, got \
             {patched}"
        );
        assert_eq!(
            content_member(&patched, "title"),
            Some("Patched".to_string()),
            "case {case:?}: expected the patched title, got {patched}"
        );
    }
}

#[tokio::test]
async fn a_page_is_patched_only_through_its_own_product_and_minted_id() {
    let emulator = emulator();
    let case = "a patch addressed at a page nothing minted";

    let (uri, created) =
        page_under_a_fresh_product(&emulator, "Addressed", "The only page", "The only body").await;
    let page_id = as_str(&created, "productPageId");
    let collection = uri
        .rsplit_once('/')
        .map(|(collection, _)| collection.to_string())
        .unwrap_or_default();
    let before = get_page(&emulator, case, &uri).await;

    // A second product, which holds a page of its own, so the first product's id is not
    // reachable through it even where both products minted the same id.
    let other_product = product(&emulator, "Another product").await;
    let (status, other_created) = call(
        &emulator,
        "POST",
        &pages_of(&other_product),
        Some(json!({ "displayContent": { "title": "Elsewhere", "body": "Elsewhere body" } })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateProductPage under the second product, got {status} \
         {other_created}"
    );

    let misaddressed: Vec<(&str, String)> = vec![
        (
            "a product nothing minted",
            format!("{}/{page_id}", pages_of("nosuchprod0")),
        ),
        (
            "a page id nothing minted",
            format!("{collection}/nosuchpage0"),
        ),
        (
            "the page id's uppercase form",
            format!("{collection}/{}", page_id.to_uppercase()),
        ),
        (
            "the page id with its first character dropped",
            format!("{collection}/{}", &page_id[1..]),
        ),
        (
            "a page id at the modelled maximum that nothing minted",
            format!("{collection}/{}", "z".repeat(30)),
        ),
    ];

    for (label, misaddressed_uri) in misaddressed {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &misaddressed_uri,
            Some(json!({ "displayContent": { "title": "Nowhere", "body": "Nowhere body" } })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 from UpdateProductPage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {label:?}: expected NotFoundException, got {body}"
        );

        // Every operation that ADDRESSES a product page reports an identifier that
        // resolves to nothing with one sentence, so the patch's message is the one the
        // read of the same address answers.
        let (read_status, read) = call(&emulator, "GET", &misaddressed_uri, None).await;
        assert_eq!(
            read_status, 404,
            "case {label:?}: expected 404 from GetProductPage for the same address, got \
             {read_status} {read}"
        );
        assert_eq!(
            as_str(&body, "message"),
            as_str(&read, "message"),
            "case {label:?}: expected the sentence GetProductPage refuses this address with, \
             got {:?}",
            as_str(&body, "message")
        );
    }

    // None of those refusals wrote anything: the page that does exist is unchanged, and
    // the second product's page still carries its own content.
    let after = get_page(&emulator, case, &uri).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the existing page to be untouched by the refused patches, got \
         {after} against {before}"
    );
    let other_uri = format!(
        "{}/{}",
        pages_of(&other_product),
        as_str(&other_created, "productPageId")
    );
    let other_after = get_page(&emulator, "the second product's page", &other_uri).await;
    assert_eq!(
        content_member(&other_after, "title"),
        Some("Elsewhere".to_string()),
        "expected the second product's page to keep its own content, got {other_after}"
    );
}

#[tokio::test]
async fn a_display_content_the_model_does_not_allow_is_refused_and_persists_nothing() {
    let emulator = emulator();
    let case = "an invalid patch";

    let (uri, _created) =
        page_under_a_fresh_product(&emulator, "Refused content", "A valid title", "A valid body")
            .await;
    let before = get_page(&emulator, case, &uri).await;

    // Every row fails a rule the model declares for `DisplayContent`: a required member
    // absent, a member below its minimum length (1), or one character past its published
    // maximum.
    let rows: Vec<(&str, Value)> = vec![
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

    for (label, patch) in rows {
        let (status, body) = call(&emulator, "PATCH", &uri, Some(patch)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from UpdateProductPage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {label:?}: expected BadRequestException, got {body}"
        );

        let after = get_page(&emulator, label, &uri).await;
        assert_eq!(
            after, before,
            "case {label:?}: expected a refused patch to persist nothing, got {after} against \
             {before}"
        );
    }

    // A patch the model accepts still lands after the refusals.
    let (status, patched) = call(
        &emulator,
        "PATCH",
        &uri,
        Some(json!({ "displayContent": { "title": "Accepted", "body": "Accepted body" } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from UpdateProductPage after the refusals, got {status} {patched}"
    );
    let fetched = get_page(&emulator, "after the refusals", &uri).await;
    assert_eq!(
        patchable(&fetched),
        patchable(&patched),
        "expected GetProductPage to report the accepted patch, got {fetched} against {patched}"
    );
}
