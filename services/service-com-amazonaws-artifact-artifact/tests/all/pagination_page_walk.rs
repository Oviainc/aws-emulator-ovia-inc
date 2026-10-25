// SPDX-License-Identifier: CC0-1.0

//! RULE — every paginated listing of this service walks its collection by ONE rule, and
//! this suite drives that one rule through all five of them.
//!
//! The rule: a request carrying no `nextToken` starts at the beginning; a token the
//! operation itself cut resumes at the number of entries it has already REPORTED; the page
//! holds at most `maxResults` entries; and a `nextToken` is cut exactly when a further
//! entry remains — so walking a listing a page at a time, at ANY admissible page size,
//! reports every record once in the order one unpaged call reports them, and the last page
//! of a walk carries no token. The other half of the same rule is the refusal: a value the
//! operation did not cut is not an offset, so it is answered with the service's one fixed
//! invalid-token `ValidationException` rather than honoured.
//!
//! The rows span every collection this service pages, which is what the rule generalises
//! over: a service-owned catalog (`ListReports`), the sub-collection of one catalog
//! resource (`ListReportVersions`), the calling account's own set (`ListCustomerAgreements`),
//! the records the account CREATED through this API (`ListComplianceInquiries`) and the
//! sub-collection of one created record (`ListComplianceInquiryQueries`) — read-only state
//! and caller-written state, filtered collections and unfiltered ones, whole collections
//! and scoped ones. Each is walked at both published bounds of `MaxResultsAttribute`'s
//! `@range(min: 1, max: 300)` and at sizes between them that cut it in different places.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, get, signed_for, Answer, REGION};

/// The report whose versions the scoped report row pages — a catalog row publishing
/// several versions, so the row really pages.
const SCOPED_REPORT: &str = "report-5eevElBv6MyRmQsD";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// The sections the paged inquiry's document names. An upload is processed section by
/// section, so the inquiry owns one query per section and its query listing pages.
const FILE_SECTIONS: &[&str] = &["Sheet1", "Sheet2", "Sheet3", "Sheet4"];

/// The page sizes every listing is walked at: the floor and the ceiling of
/// `MaxResultsAttribute`'s `@range(min: 1, max: 300)`
/// (`smithy_query(query="shape",shape="MaxResultsAttribute")`), and sizes between them
/// that cut the collections at different places.
const PAGE_SIZES: &[usize] = &[1, 2, 3, 300];

/// The fixed text this service answers for a token it did not issue. Spelled out here
/// rather than imported from the crate, so a changed constant fails this suite instead of
/// redefining what it asserts.
const INVALID_TOKEN: &str = "The provided token is invalid.";

/// One paginated listing of this service.
struct Listing {
    /// The operation, as a case label.
    case: &'static str,
    /// Its modelled `@http` URI.
    path: &'static str,
    /// The query this operation needs before `maxResults` and `nextToken`: the resource a
    /// SCOPED listing pages the sub-collection of, or nothing for a whole collection.
    scope: String,
    /// The output member whose list the page cuts.
    items: &'static str,
    /// The member of one reported record that names exactly that record, so two pages can
    /// be compared whatever collection they came from.
    key: &'static str,
}

/// Create one compliance inquiry and return the `InquiryId` it minted.
async fn create_inquiry(router: &MultiServiceRouter, body: &Value) -> String {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/v1/compliance-inquiry/create")
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", signed_for(REGION))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(
        status, 202,
        "case {:?}: creating an inquiry must be accepted, got {status} with {body:?}",
        "CreateComplianceInquiry"
    );
    body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create must report an inquiry id, got {body:?}"))
        .to_string()
}

/// The five paginated listings, over an emulator seeded with the inquiries the last two
/// of them page. The `@http` URIs and output members are the ones the model binds
/// (`smithy_query(query="digest")`).
async fn listings(router: &MultiServiceRouter) -> Vec<Listing> {
    // Several inquiries, differing in the form of the submission, so the account's set
    // pages; the document with sections is the one whose queries page.
    let paged = create_inquiry(
        router,
        &json!({
            "name": "Annual audit workbook",
            "inquiryContent": { "fileContent": { "content": FILE_CONTENT, "fileSections": FILE_SECTIONS } },
        }),
    )
    .await;
    for name in ["Data residency", "Subprocessors", "Encryption at rest"] {
        create_inquiry(
            router,
            &json!({ "name": name, "inquiryContent": { "query": format!("What about {name}?") } }),
        )
        .await;
    }

    vec![
        Listing {
            case: "ListReports",
            path: "/v1/report/list",
            scope: String::new(),
            items: "reports",
            key: "arn",
        },
        Listing {
            case: "ListReportVersions",
            path: "/v1/report/listVersions",
            scope: format!("reportId={SCOPED_REPORT}"),
            items: "reports",
            key: "arn",
        },
        Listing {
            case: "ListCustomerAgreements",
            path: "/v1/customer-agreement/list",
            scope: String::new(),
            items: "customerAgreements",
            key: "arn",
        },
        Listing {
            case: "ListComplianceInquiries",
            path: "/v1/compliance-inquiry/list",
            scope: String::new(),
            items: "complianceInquiries",
            key: "id",
        },
        Listing {
            case: "ListComplianceInquiryQueries",
            path: "/v1/compliance-inquiry/listQueries",
            scope: format!("complianceInquiryId={paged}"),
            items: "queries",
            key: "queryIdentifier",
        },
    ]
}

/// One page of `listing`, with `query` appended after whatever scope the listing needs.
async fn page(router: &MultiServiceRouter, listing: &Listing, query: &str) -> Answer {
    let mut uri = listing.path.to_string();
    let mut separator = '?';
    for part in [listing.scope.as_str(), query] {
        if part.is_empty() {
            continue;
        }
        uri.push(separator);
        uri.push_str(part);
        separator = '&';
    }
    get(router, &uri).await
}

/// What each record on this page names itself as — the member that identifies exactly one
/// record of the listing's collection.
fn identities(answer: &Answer, listing: &Listing) -> Vec<String> {
    answer.body[listing.items]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let named = &row[listing.key];
                    assert!(
                        !named.is_null(),
                        "case {:?}: every reported record carries {:?}, got {row:?}",
                        listing.case,
                        listing.key
                    );
                    named.to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn every_paginated_listing_walks_its_pages_by_the_one_shared_rule() {
    let router = emulator();

    for listing in listings(&router).await {
        // What one unpaged call reports is the order and the content every walk below has
        // to reproduce — the rule, rather than one recorded page.
        let whole = page(&router, &listing, "").await;
        assert_eq!(
            whole.status, 200,
            "case {:?}: an unpaged listing must be served, got {} with {:?}",
            listing.case, whole.status, whole.body
        );
        let expected = identities(&whole, &listing);
        assert!(
            expected.len() > 1,
            "case {:?}: the rule needs several records to page over, got {expected:?}",
            listing.case
        );
        assert!(
            whole.next_token().is_none(),
            "case {:?}: a page holding every record promises no further one, so no \
             nextToken is owed, got {:?}",
            listing.case,
            whole.next_token()
        );

        for page_size in PAGE_SIZES {
            let case = format!("{} maxResults={page_size}", listing.case);
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut calls = 0usize;
            loop {
                calls += 1;
                assert!(
                    calls <= expected.len() + 1,
                    "case {case:?}: paging must terminate, got {calls} calls for {} records",
                    expected.len()
                );
                let query = match token.as_deref() {
                    None => format!("maxResults={page_size}"),
                    Some(value) => format!("maxResults={page_size}&nextToken={value}"),
                };
                let answer = page(&router, &listing, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a cursor this operation cut must be served, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                let rows = identities(&answer, &listing);
                assert!(
                    rows.len() <= *page_size,
                    "case {case:?}: a page is cut at maxResults, got {} records: {rows:?}",
                    rows.len()
                );
                walked.extend(rows.iter().cloned());
                match answer.next_token() {
                    None => break,
                    Some(issued) => {
                        assert_eq!(
                            rows.len(),
                            *page_size,
                            "case {case:?}: a nextToken promises a further record, so it is \
                             only cut on a full page, got {} records: {rows:?}",
                            rows.len()
                        );
                        token = Some(issued);
                    }
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?}: walking the pages must report every record once, in the \
                 order one unpaged call reports them, got {walked:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_paginated_listing_refuses_a_value_it_did_not_cut() {
    let router = emulator();
    let listings = listings(&router).await;

    // The cursor each listing really cuts, so every one of them can be handed to the four
    // operations that did not cut it.
    let mut issued: Vec<String> = Vec::new();
    for listing in &listings {
        let token = page(&router, listing, "maxResults=1")
            .await
            .next_token()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a collection of several records must page",
                    listing.case
                )
            });
        issued.push(token);
    }

    for (index, listing) in listings.iter().enumerate() {
        let own = issued[index].as_str();
        let mut rows: Vec<(String, String)> = vec![
            (
                "a value that is not a cursor".to_string(),
                "not-a-token".to_string(),
            ),
            ("a bare position".to_string(), "1".to_string()),
            (
                "hex that is not a cursor".to_string(),
                ::hex::encode("artifact"),
            ),
            (
                "its own cursor truncated".to_string(),
                // By character, not by byte offset: a codec that ever emits a token
                // outside ASCII must fail these assertions rather than this slice.
                own.chars()
                    .take(own.chars().count().saturating_sub(2))
                    .collect(),
            ),
        ];
        for (other, token) in issued.iter().enumerate() {
            if other != index {
                rows.push((
                    format!("the cursor {} cut", listings[other].case),
                    token.clone(),
                ));
            }
        }

        for (what, token) in rows {
            let case = format!("{} given {what}", listing.case);
            let refused = page(&router, listing, &format!("nextToken={token}")).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: a value this operation did not cut must be refused, got {} \
                 with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.body["message"].as_str().unwrap_or_default(),
                INVALID_TOKEN,
                "case {case:?}: expected the service's one invalid-token text, got {:?}",
                refused.body
            );
        }

        // The cursor this listing DID cut is served, at the same position, however many
        // times it is spent: the walk reads the token, it never consumes it.
        let resumed = page(&router, listing, &format!("maxResults=1&nextToken={own}")).await;
        let again = page(&router, listing, &format!("maxResults=1&nextToken={own}")).await;
        assert_eq!(
            resumed.status, 200,
            "case {:?}: its own cursor must be served, got {} with {:?}",
            listing.case, resumed.status, resumed.body
        );
        assert_eq!(
            identities(&resumed, listing),
            identities(&again, listing),
            "case {:?}: spending a cursor twice must report the same page, got {:?} then {:?}",
            listing.case,
            resumed.body,
            again.body
        );
    }
}
