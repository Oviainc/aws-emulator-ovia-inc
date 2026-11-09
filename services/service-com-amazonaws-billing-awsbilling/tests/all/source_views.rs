// SPDX-License-Identifier: CC0-1.0

//! `ListSourceViewsForBillingView`, end to end over the awsJson1_0 wire.
//!
//! Every assertion here is about the RULE the operation follows: it reports
//! exactly the source-view ARNs persisted on the billing view the request names,
//! in the order that view stores them, sliced by `maxResults` and resumed by the
//! `nextToken` the previous page returned — and it reports nothing else, so an
//! ARN no view is stored under is refused with the modeled
//! `ResourceNotFoundException` and a view holding no source views answers an
//! empty list.
//!
//! The table rows differ along what the rule generalises over: how many source
//! views a view holds (1, the `BillingViewSourceViewsList` minimum, through 10,
//! its maximum), the order they were associated in, and the page size the caller
//! asks for.

use serde_json::{json, Value};

// The awsJson1_0 harness — a router over ONE shared engine, plus `call` and
// `error_code` — already exists for the billing-view lifecycle suite, so this
// module reuses it instead of building a second one.
use crate::billing_views::{call, emulator, error_code, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN};

/// A pattern-valid `BillingViewArn` for a source view with resource id `id`.
fn source_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

/// One row of the source-view table: the `sourceViews` a create persists, which
/// is exactly what the list must report back.
struct SourceViewRow {
    case: &'static str,
    source_views: Vec<String>,
}

/// The rows: source-view counts from the modeled minimum of 1 to the modeled
/// maximum of 10 (`BillingViewSourceViewsList`, len 1..10), in associated order
/// rather than ARN order, so a sorted answer would fail.
fn source_view_rows() -> Vec<SourceViewRow> {
    vec![
        SourceViewRow {
            case: "one source view",
            source_views: vec![PRIMARY_ARN.to_string()],
        },
        SourceViewRow {
            case: "two, in descending ARN order",
            source_views: vec![source_arn("src-b"), source_arn("src-a")],
        },
        SourceViewRow {
            case: "three, neither ascending nor descending",
            source_views: vec![source_arn("m-src"), PRIMARY_ARN.to_string(), source_arn("a-src")],
        },
        SourceViewRow {
            case: "ten, the modeled maximum, in descending order",
            source_views: (0..10)
                .rev()
                .map(|index| source_arn(&format!("src-{index}")))
                .collect(),
        },
    ]
}

/// The `sourceViews` an answer carries. A protocol that omits an empty list and
/// one that writes `[]` mean the same thing, so both read back as no entries.
fn source_views(body: &Value) -> Vec<String> {
    body.get("sourceViews")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create a billing view holding `source_views` and return its minted ARN.
async fn create_view(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
    source_views: &[String],
) -> String {
    let (status, created) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": source_views }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateBillingView to answer 200, got {status} {created:?}"
    );
    created["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn every_persisted_source_view_is_reported_in_stored_order() {
    let router = emulator();

    for (index, row) in source_view_rows().iter().enumerate() {
        let case = row.case;
        let arn = create_view(&router, case, &format!("sv-view-{index}"), &row.source_views).await;

        let (status, listed) =
            call(&router, "ListSourceViewsForBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListSourceViewsForBillingView, got {status} {listed:?}"
        );
        assert_eq!(
            source_views(&listed),
            row.source_views,
            "case {case:?}: expected the stored source views, got {listed:?}"
        );
        assert!(
            listed.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a single page must carry no nextToken, got {:?}",
            listed.get("nextToken")
        );

        // The same stored list is what `GetBillingView` counts, so the two reads
        // cannot disagree.
        let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetBillingView, got {status} {got:?}"
        );
        assert_eq!(
            got["billingView"]["sourceViewCount"].as_u64(),
            Some(row.source_views.len() as u64),
            "case {case:?}: expected sourceViewCount {}, got {:?}",
            row.source_views.len(),
            got["billingView"]["sourceViewCount"]
        );
    }
}

#[tokio::test]
async fn pagination_returns_every_source_view_exactly_once_for_every_page_size() {
    let router = emulator();

    for (index, row) in source_view_rows().iter().enumerate() {
        let arn = create_view(
            &router,
            row.case,
            &format!("sv-page-{index}"),
            &row.source_views,
        )
        .await;

        // Every page size from 1 (one entry per page) through one past the whole
        // list (a single page), plus the modeled `maxResults` maximum of 100.
        for page_size in (1..=row.source_views.len() + 1).chain(std::iter::once(100)) {
            let case = format!("{} at page size {page_size}", row.case);
            let mut collected: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;

            loop {
                let mut request = json!({ "arn": arn, "maxResults": page_size });
                if let Some(token) = &next_token {
                    request["nextToken"] = json!(token);
                }
                let (status, page) =
                    call(&router, "ListSourceViewsForBillingView", &request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 for page {pages}, got {status} {page:?}"
                );

                let entries = source_views(&page);
                assert!(
                    entries.len() <= page_size,
                    "case {case:?}: a page must hold at most maxResults entries, got {entries:?}"
                );
                collected.extend(entries);

                next_token = page
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                pages += 1;
                assert!(
                    pages <= row.source_views.len() + 1,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                if next_token.is_none() {
                    break;
                }
            }

            assert_eq!(
                collected, row.source_views,
                "case {case:?}: expected every stored source view exactly once in order, got {collected:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_view_holding_no_source_views_answers_an_empty_list() {
    let router = emulator();

    // The account's PRIMARY view draws on no other view, so it holds no source
    // views; every page size must agree that there are none.
    for request in [
        json!({ "arn": PRIMARY_ARN }),
        json!({ "arn": PRIMARY_ARN, "maxResults": 1 }),
        json!({ "arn": PRIMARY_ARN, "maxResults": 100 }),
    ] {
        let case = request.to_string();
        let (status, listed) = call(&router, "ListSourceViewsForBillingView", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for a view with no source views, got {status} {listed:?}"
        );
        assert!(
            source_views(&listed).is_empty(),
            "case {case:?}: expected no source views, got {listed:?}"
        );
        assert!(
            listed.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: an empty answer must carry no nextToken, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_no_view_is_stored_under_is_refused_with_the_modeled_error() {
    let router = emulator();

    // Pattern-valid `BillingViewArn`s (the boundary validator already refuses
    // anything else) that name no stored view of the caller's own account: a
    // deleted one, one that never existed, and one whose resource id is a nested
    // path. An ARN of ANOTHER account never reaches this lookup — the account
    // boundary is enforced first, which `tests/all/arn_account_boundary.rs`
    // pins for every ARN-addressed operation.
    let deleted = create_view(
        &router,
        "deleted view",
        "sv-deleted",
        &[PRIMARY_ARN.to_string()],
    )
    .await;
    let (status, deleted_body) = call(&router, "DeleteBillingView", &json!({ "arn": deleted })).await;
    assert_eq!(
        status, 200,
        "case \"delete\": expected 200, got {status} {deleted_body:?}"
    );

    let cases: Vec<(&str, String)> = vec![
        ("never created", ABSENT_ARN.to_string()),
        ("deleted since", deleted),
        (
            "a nested resource id",
            format!("arn:aws:billing::{ACCOUNT_ID}:billingview/never/created"),
        ),
    ];

    for (case, arn) in cases {
        let (status, refused) =
            call(&router, "ListSourceViewsForBillingView", &json!({ "arn": arn })).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
    }

    // The same "no view is stored under it" ARN in ANOTHER account is a
    // different answer, because the account boundary is decided first: the
    // refusal may not disclose whether the other account holds a view.
    let (status, refused) = call(
        &router,
        "ListSourceViewsForBillingView",
        &json!({ "arn": "arn:aws:billing::123456789012:billingview/primary" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "case \"another account\": expected 403 AccessDeniedException, got {status} {refused:?}"
    );
}

#[tokio::test]
async fn a_request_the_service_cannot_honour_is_refused_with_a_validation_error() {
    let router = emulator();
    let arn = create_view(
        &router,
        "validation",
        "sv-validation",
        &[source_arn("src-a"), source_arn("src-b")],
    )
    .await;

    // Each row breaks one rule: a `nextToken` this service never issued (it
    // decodes to nothing it writes), or a `maxResults` outside the modeled
    // range 1..100.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a nextToken that is not hex",
            json!({ "arn": arn, "nextToken": "notatoken" }),
        ),
        (
            "a nextToken that decodes to text, not a position",
            json!({ "arn": arn, "nextToken": hex::encode("abc") }),
        ),
        (
            "a nextToken that decodes to a negative position",
            json!({ "arn": arn, "nextToken": hex::encode("-1") }),
        ),
        (
            "maxResults below its range minimum",
            json!({ "arn": arn, "maxResults": 0 }),
        ),
        (
            "maxResults above its range maximum",
            json!({ "arn": arn, "maxResults": 101 }),
        ),
    ];

    for (case, request) in cases {
        let (status, refused) = call(&router, "ListSourceViewsForBillingView", &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {refused:?}"
        );
    }

    // A token past the end of the list is one this service issued for a page
    // whose entries have since gone: it is the last page, not a bad request.
    let (status, listed) = call(
        &router,
        "ListSourceViewsForBillingView",
        &json!({ "arn": arn, "nextToken": hex::encode("9") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a token past the end\": expected 200, got {status} {listed:?}"
    );
    assert!(
        source_views(&listed).is_empty(),
        "case \"a token past the end\": expected no entries, got {listed:?}"
    );
}

#[tokio::test]
async fn listing_the_source_views_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let stored = vec![source_arn("src-b"), PRIMARY_ARN.to_string(), source_arn("src-a")];
    let arn = create_view(&router, "repeated reads", "sv-repeat", &stored).await;

    // The same first page three times over, then the whole list: a read neither
    // consumes an entry nor advances a cursor of its own.
    for attempt in 0..3 {
        let case = format!("first page, attempt {attempt}");
        let (status, page) = call(
            &router,
            "ListSourceViewsForBillingView",
            &json!({ "arn": arn, "maxResults": 1 }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {page:?}");
        assert_eq!(
            source_views(&page),
            stored[..1].to_vec(),
            "case {case:?}: expected the same first page every time, got {page:?}"
        );
    }

    let (status, listed) =
        call(&router, "ListSourceViewsForBillingView", &json!({ "arn": arn })).await;
    assert_eq!(status, 200, "case \"full list\": got {status} {listed:?}");
    assert_eq!(
        source_views(&listed),
        stored,
        "case \"full list\": expected the stored list unchanged by the reads, got {listed:?}"
    );

    let (status, got) = call(&router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(status, 200, "case \"get after reads\": got {status} {got:?}");
    assert_eq!(
        got["billingView"]["sourceViewCount"].as_u64(),
        Some(stored.len() as u64),
        "case \"get after reads\": expected sourceViewCount {}, got {:?}",
        stored.len(),
        got["billingView"]["sourceViewCount"]
    );
}
