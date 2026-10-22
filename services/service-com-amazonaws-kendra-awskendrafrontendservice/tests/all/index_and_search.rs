// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Kendra index-and-search loop: `CreateIndex`,
//! `DescribeIndex`, `ListIndices`, `BatchPutDocument`, `Query` and `DeleteIndex`.
//!
//! Every call goes over the wire the way an SDK client makes it — `POST /` with
//! `X-Amz-Target: AWSKendraFrontendService.<Operation>` and an `awsJson1_1` body —
//! through the same `MultiServiceRouter::dispatch` the emulator's front door uses,
//! so what is asserted here is the service's HTTP behaviour and not an internal API.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// An IAM role ARN matching the modelled `RoleArn` pattern.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/kendra";

/// An `IndexId` (36 characters, `^[a-zA-Z0-9][a-zA-Z0-9-]*$`) that was never created.
const ABSENT_INDEX_ID: &str = "00000000-0000-0000-0000-000000000000";

/// A router with just this service mounted on a fresh engine, so each test observes
/// only its own state.
fn kendra() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the core's business and is off for these tests; the
    // `Authorization` header below is still what routes the request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kendra_awskendrafrontendservice::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and decoded JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSKendraFrontendService.{operation}"),
        )
        // The SigV4 credential scope names the target service (`kendra`), which is
        // how a signed request routes.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-west-2/kendra/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// The wire error code of an error response (`__type` on the awsJson protocols).
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create an index and return its `Id`.
async fn create_index(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateIndex",
        json!({ "Name": name, "RoleArn": ROLE_ARN, "Edition": "DEVELOPER_EDITION" }),
    )
    .await;
    assert_eq!(status, 200, "CreateIndex({name}): got {body:?}");
    body["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIndex({name}) returned no Id: {body:?}"))
        .to_string()
}

/// Wait out the activation span `CreateIndex` scheduled, polling `DescribeIndex` the
/// way a client does. Index creation is asynchronous, so this is the documented way
/// to learn the index is ready: "The `Status` field is set to `ACTIVE` when the index
/// is ready to use."
async fn wait_until_active(router: &MultiServiceRouter, index_id: &str) {
    for _ in 0..100 {
        let (status, body) = call(router, "DescribeIndex", json!({ "Id": index_id })).await;
        assert_eq!(status, 200, "DescribeIndex({index_id}): got {body:?}");
        if body["Status"] == json!("ACTIVE") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("index {index_id} never became ACTIVE");
}

/// Put one text document into an index and assert it was accepted.
async fn put_text_document(
    router: &MultiServiceRouter,
    index_id: &str,
    id: &str,
    title: &str,
    body_text: &str,
) {
    let (status, body) = call(
        router,
        "BatchPutDocument",
        json!({
            "IndexId": index_id,
            "Documents": [{
                "Id": id,
                "Title": title,
                "Blob": aws_smithy_types::base64::encode(body_text),
                "ContentType": "PLAIN_TEXT",
            }],
        }),
    )
    .await;
    assert_eq!(status, 200, "BatchPutDocument({id}): got {body:?}");
    assert_eq!(
        body["FailedDocuments"],
        json!([]),
        "BatchPutDocument({id}): expected no failed documents, got {body:?}"
    );
}

/// The whole loop: create, describe, list, put, query, delete — and prove each step
/// reads back what the previous one wrote.
#[tokio::test]
async fn the_index_and_search_loop_round_trips_through_stored_state() {
    let router = kendra();

    let (status, created) = call(
        &router,
        "CreateIndex",
        json!({
            "Name": "demo-index",
            "RoleArn": ROLE_ARN,
            "Edition": "DEVELOPER_EDITION",
            "Description": "the bootstrap slice",
            "UserContextPolicy": "ATTRIBUTE_FILTER",
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateIndex: got {created:?}");
    let index_id = created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIndex returned no Id: {created:?}"))
        .to_string();
    assert_eq!(
        index_id.chars().count(),
        36,
        "IndexId is modelled len 36..36, got {index_id:?}"
    );
    assert!(
        index_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && index_id.starts_with(|c: char| c.is_ascii_alphanumeric()),
        "IndexId must match ^[a-zA-Z0-9][a-zA-Z0-9-]*$, got {index_id:?}"
    );

    // Index creation is asynchronous, so a caller acting straight away sees CREATING.
    let (status, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(status, 200, "DescribeIndex while creating: got {described:?}");
    assert_eq!(
        described["Status"],
        json!("CREATING"),
        "a freshly created index is CREATING, got {described:?}"
    );

    // And documents cannot be added to an index that is not ready yet.
    let (status, refused) = call(
        &router,
        "BatchPutDocument",
        json!({
            "IndexId": index_id,
            "Documents": [{ "Id": "doc-early", "Blob": aws_smithy_types::base64::encode("too soon") }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (409, "ConflictException"),
        "BatchPutDocument on a CREATING index: got {status} {refused:?}"
    );

    wait_until_active(&router, &index_id).await;

    let (status, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(status, 200, "DescribeIndex: got {described:?}");
    for (member, expected) in [
        ("Id", json!(index_id.clone())),
        ("Name", json!("demo-index")),
        ("Edition", json!("DEVELOPER_EDITION")),
        ("RoleArn", json!(ROLE_ARN)),
        ("Description", json!("the bootstrap slice")),
        ("UserContextPolicy", json!("ATTRIBUTE_FILTER")),
        ("Status", json!("ACTIVE")),
    ] {
        assert_eq!(
            described[member], expected,
            "DescribeIndex.{member}: expected {expected:?}, got {:?}",
            described[member]
        );
    }
    assert!(
        described["CreatedAt"].is_number() && described["UpdatedAt"].is_number(),
        "DescribeIndex must report CreatedAt/UpdatedAt, got {described:?}"
    );
    let updated_before_ingest = described["UpdatedAt"].as_f64().unwrap_or_default();
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextDocumentsCount"],
        json!(0),
        "an index with no documents counts none, got {described:?}"
    );
    assert_eq!(
        described["IndexStatistics"]["FaqStatistics"]["IndexedQuestionAnswersCount"],
        json!(0),
        "no FAQ can be indexed yet, got {described:?}"
    );

    // ListIndices lists exactly the index that was created.
    let (status, listed) = call(&router, "ListIndices", json!({})).await;
    assert_eq!(status, 200, "ListIndices: got {listed:?}");
    let items = listed["IndexConfigurationSummaryItems"]
        .as_array()
        .unwrap_or_else(|| panic!("ListIndices returned no items: {listed:?}"));
    assert_eq!(items.len(), 1, "ListIndices: got {listed:?}");
    assert_eq!(items[0]["Id"], json!(index_id.clone()), "{listed:?}");
    assert_eq!(items[0]["Name"], json!("demo-index"), "{listed:?}");
    assert_eq!(items[0]["Status"], json!("ACTIVE"), "{listed:?}");

    let body_text = "the quick brown fox jumps over the lazy dog";
    put_text_document(
        &router,
        &index_id,
        "doc-1",
        "Kendra bootstrap guide",
        body_text,
    )
    .await;

    let (_, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextDocumentsCount"],
        json!(1),
        "the stored document must be counted, got {described:?}"
    );
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextBytes"],
        json!(body_text.len()),
        "the indexed bytes are the stored text's, got {described:?}"
    );
    assert!(
        described["UpdatedAt"].as_f64().unwrap_or_default() >= updated_before_ingest,
        "ingesting documents changes the index, so UpdatedAt must not go backwards: \
         {updated_before_ingest} then {:?}",
        described["UpdatedAt"]
    );

    // A term the document carries finds it, with an excerpt drawn from the stored body.
    let (status, found) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "brown fox" }),
    )
    .await;
    assert_eq!(status, 200, "Query: got {found:?}");
    assert!(
        found["QueryId"].as_str().is_some_and(|id| !id.is_empty()),
        "Query must mint a QueryId, got {found:?}"
    );
    assert_eq!(
        found["TotalNumberOfResults"],
        json!(1),
        "one document matches, got {found:?}"
    );
    let results = found["ResultItems"]
        .as_array()
        .unwrap_or_else(|| panic!("Query returned no ResultItems: {found:?}"));
    assert_eq!(results.len(), 1, "Query: got {found:?}");
    assert_eq!(results[0]["DocumentId"], json!("doc-1"), "{found:?}");
    assert_eq!(results[0]["Type"], json!("DOCUMENT"), "{found:?}");
    assert_eq!(
        results[0]["DocumentTitle"]["Text"],
        json!("Kendra bootstrap guide"),
        "{found:?}"
    );
    assert_eq!(
        results[0]["DocumentExcerpt"]["Text"],
        json!(body_text),
        "the excerpt is drawn from the stored body, got {found:?}"
    );
    assert_eq!(
        results[0]["ScoreAttributes"]["ScoreConfidence"],
        json!("VERY_HIGH"),
        "every query term matched, got {found:?}"
    );

    // Deletion is asynchronous as well: DELETING first, then gone.
    let (status, deleted) = call(&router, "DeleteIndex", json!({ "Id": index_id })).await;
    assert_eq!(status, 200, "DeleteIndex: got {deleted:?}");
    let (status, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(status, 200, "DescribeIndex while deleting: got {described:?}");
    assert_eq!(
        described["Status"],
        json!("DELETING"),
        "a deleted index reports DELETING until it is gone, got {described:?}"
    );

    for _ in 0..100 {
        let (status, body) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
        if status != 200 {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "DescribeIndex after deletion: got {status} {body:?}"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let (status, body) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "a deleted index is gone: got {status} {body:?}"
    );
    let (status, body) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "fox" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "a deleted index cannot be queried: got {status} {body:?}"
    );
    let (status, listed) = call(&router, "ListIndices", json!({})).await;
    assert_eq!(status, 200, "ListIndices: got {listed:?}");
    assert_eq!(
        listed["IndexConfigurationSummaryItems"],
        json!([]),
        "a deleted index is not listed, got {listed:?}"
    );
}

/// One query term class per row: the result set is computed from what each document
/// carries, never canned.
#[tokio::test]
async fn a_query_returns_exactly_the_documents_carrying_its_terms() {
    let router = kendra();
    let index_id = create_index(&router, "search-index").await;
    wait_until_active(&router, &index_id).await;

    put_text_document(
        &router,
        &index_id,
        "doc-fox",
        "Animal notes",
        "the quick brown fox jumps over the lazy dog",
    )
    .await;
    put_text_document(
        &router,
        &index_id,
        "doc-kendra",
        "Kendra bootstrap guide",
        "an index is created, then documents are put into it",
    )
    .await;
    put_text_document(
        &router,
        &index_id,
        "doc-both",
        "Dog training",
        "a kendra index can hold notes about a dog",
    )
    .await;

    // (query text, expected document ids in result order, expected total)
    let cases: &[(&str, &[&str], i64)] = &[
        // A term in one document's body.
        ("fox", &["doc-fox"], 1),
        // A term two documents share, the title match ranked first.
        ("kendra", &["doc-kendra", "doc-both"], 2),
        // Case-insensitive: the stored text is lowercase, the query is not.
        ("KENDRA", &["doc-kendra", "doc-both"], 2),
        // Several terms: a document carrying both outranks one carrying one.
        ("dog kendra", &["doc-both", "doc-kendra", "doc-fox"], 3),
        // Punctuation is a term separator, not part of a term.
        ("created,", &["doc-kendra"], 1),
        // A term no document carries matches nothing.
        ("zzz-no-such-term", &[], 0),
        // A term that is only a substring of a stored word is not that word.
        ("do", &[], 0),
    ];

    for (query_text, expected_ids, expected_total) in cases {
        let (status, body) = call(
            &router,
            "Query",
            json!({ "IndexId": index_id, "QueryText": query_text }),
        )
        .await;
        assert_eq!(status, 200, "Query({query_text:?}): got {body:?}");
        assert_eq!(
            body["TotalNumberOfResults"],
            json!(*expected_total),
            "Query({query_text:?}): expected total {expected_total}, got {body:?}"
        );
        let actual_ids: Vec<&str> = body["ResultItems"]
            .as_array()
            .unwrap_or_else(|| panic!("Query({query_text:?}) returned no ResultItems: {body:?}"))
            .iter()
            .map(|item| item["DocumentId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            actual_ids, *expected_ids,
            "Query({query_text:?}): expected {expected_ids:?}, got {actual_ids:?}"
        );
    }

    // A result type drawn from FAQs matches nothing: no FAQ can be indexed yet.
    for result_type in ["QUESTION_ANSWER", "ANSWER"] {
        let (status, body) = call(
            &router,
            "Query",
            json!({
                "IndexId": index_id,
                "QueryText": "kendra",
                "QueryResultTypeFilter": result_type,
            }),
        )
        .await;
        assert_eq!(status, 200, "Query({result_type}): got {body:?}");
        assert_eq!(
            (
                body["TotalNumberOfResults"].clone(),
                body["ResultItems"].clone()
            ),
            (json!(0), json!([])),
            "Query({result_type}): expected no results, got {body:?}"
        );
    }

    // The highlight offsets point at the query term inside the text returned.
    let (_, body) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "fox" }),
    )
    .await;
    let excerpt = body["ResultItems"][0]["DocumentExcerpt"]["Text"]
        .as_str()
        .unwrap_or_else(|| panic!("no excerpt: {body:?}"));
    let highlight = &body["ResultItems"][0]["DocumentExcerpt"]["Highlights"][0];
    let begin = highlight["BeginOffset"].as_u64().unwrap_or_default() as usize;
    let end = highlight["EndOffset"].as_u64().unwrap_or_default() as usize;
    let highlighted: String = excerpt.chars().take(end).skip(begin).collect();
    assert_eq!(
        highlighted, "fox",
        "the highlight must bracket the matched term in {excerpt:?}, got {highlighted:?}"
    );
}

/// An excerpt is cut out of a body too long to return whole, at character offsets —
/// so a multi-byte body is handled as text, not as bytes.
#[tokio::test]
async fn an_excerpt_is_cut_from_a_long_body_at_character_offsets() {
    let router = kendra();
    let index_id = create_index(&router, "excerpt-index").await;
    wait_until_active(&router, &index_id).await;

    // Each row is a body longer than one excerpt, differing in where the matching
    // term sits and in how wide its characters are.
    let filler_ascii = "the quick brown fox jumps over the lazy dog ".repeat(20);
    let filler_multibyte = "日本語のテキストです ".repeat(40);
    let cases: &[(&str, String)] = &[
        // ASCII, the term near the start.
        ("doc-ascii-head", format!("kendra {filler_ascii}")),
        // ASCII, the term far past the excerpt window.
        ("doc-ascii-tail", format!("{filler_ascii} kendra")),
        // Multi-byte text around the term: every character is 3 bytes.
        (
            "doc-multibyte",
            format!("{filler_multibyte} kendra {filler_multibyte}"),
        ),
        // Multi-byte text with the term at the very end.
        (
            "doc-multibyte-tail",
            format!("{filler_multibyte}{filler_multibyte} kendra"),
        ),
    ];

    for (id, body_text) in cases {
        assert!(
            body_text.chars().count() > 300,
            "case {id}: the body must be longer than one excerpt"
        );
        put_text_document(&router, &index_id, id, "A title", body_text).await;
    }

    let (status, found) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "kendra" }),
    )
    .await;
    assert_eq!(status, 200, "Query: got {found:?}");
    let results = found["ResultItems"]
        .as_array()
        .unwrap_or_else(|| panic!("no ResultItems: {found:?}"))
        .clone();
    assert_eq!(
        results.len(),
        cases.len(),
        "every document carries the term: got {found:?}"
    );

    for item in &results {
        let id = item["DocumentId"].as_str().unwrap_or_default();
        let excerpt = item["DocumentExcerpt"]["Text"]
            .as_str()
            .unwrap_or_else(|| panic!("case {id}: no excerpt in {item:?}"));
        let excerpt_chars = excerpt.chars().count();
        assert!(
            excerpt_chars > 0 && excerpt_chars <= 300,
            "case {id}: the excerpt must be a non-empty window of at most 300 characters, \
             got {excerpt_chars}"
        );
        // The window must be cut around the matching term, not just anywhere in the
        // body — otherwise the highlight loop below would have nothing to check.
        assert!(
            excerpt.contains("kendra"),
            "case {id}: the excerpt must carry the matched term, got {excerpt:?}"
        );
        let highlights = item["DocumentExcerpt"]["Highlights"]
            .as_array()
            .unwrap_or_else(|| panic!("case {id}: no highlights in {item:?}"))
            .clone();
        assert!(
            !highlights.is_empty(),
            "case {id}: the matched term must be highlighted in {excerpt:?}"
        );
        // The highlight offsets are character offsets INTO THE EXCERPT, so slicing
        // the excerpt by them must give the term back.
        for highlight in &highlights {
            let begin = highlight["BeginOffset"].as_u64().unwrap_or_default() as usize;
            let end = highlight["EndOffset"].as_u64().unwrap_or_default() as usize;
            let highlighted: String = excerpt.chars().take(end).skip(begin).collect();
            assert_eq!(
                highlighted, "kendra",
                "case {id}: the highlight must bracket the term in {excerpt:?}, got \
                 {highlighted:?}"
            );
        }
    }
}

/// `PageSize`/`PageNumber` slice the matches; `TotalNumberOfResults` counts them all.
#[tokio::test]
async fn query_paging_slices_the_matches_without_changing_the_total() {
    let router = kendra();
    let index_id = create_index(&router, "paging-index").await;
    wait_until_active(&router, &index_id).await;

    for n in 0..5 {
        put_text_document(
            &router,
            &index_id,
            &format!("doc-{n}"),
            &format!("Report {n}"),
            "every document carries the shared term kendra",
        )
        .await;
    }

    // (page size, page number, expected document ids)
    let cases: &[(i64, i64, &[&str])] = &[
        (2, 1, &["doc-0", "doc-1"]),
        (2, 2, &["doc-2", "doc-3"]),
        (2, 3, &["doc-4"]),
        (2, 4, &[]),
        (5, 1, &["doc-0", "doc-1", "doc-2", "doc-3", "doc-4"]),
        // More than the documents stored is not an error, it is a short page.
        (100, 1, &["doc-0", "doc-1", "doc-2", "doc-3", "doc-4"]),
    ];

    for (page_size, page_number, expected_ids) in cases {
        let (status, body) = call(
            &router,
            "Query",
            json!({
                "IndexId": index_id,
                "QueryText": "kendra",
                "PageSize": page_size,
                "PageNumber": page_number,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "Query(PageSize={page_size}, PageNumber={page_number}): got {body:?}"
        );
        assert_eq!(
            body["TotalNumberOfResults"],
            json!(5),
            "the total counts every match, before paging: got {body:?}"
        );
        let actual_ids: Vec<&str> = body["ResultItems"]
            .as_array()
            .unwrap_or_else(|| panic!("no ResultItems: {body:?}"))
            .iter()
            .map(|item| item["DocumentId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            actual_ids, *expected_ids,
            "Query(PageSize={page_size}, PageNumber={page_number}): expected {expected_ids:?}, \
             got {actual_ids:?}"
        );
    }

    // A page outside the published bounds is refused, naming the member.
    for (member, value) in [("PageSize", 0), ("PageNumber", 0), ("PageSize", -1)] {
        let (status, body) = call(
            &router,
            "Query",
            json!({ "IndexId": index_id, "QueryText": "kendra", member: value }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "Query({member}={value}): got {status} {body:?}"
        );
    }
}

/// Every result of one query carries its OWN `ResultId`, whatever the document ids
/// are: `ResultId` identifies one result item, and `Document$Id` is modelled 1..2048
/// while `ResultId` is modelled 1..73, so an id cannot travel as itself.
#[tokio::test]
async fn each_result_carries_its_own_result_id() {
    let router = kendra();
    let index_id = create_index(&router, "result-id-index").await;
    wait_until_active(&router, &index_id).await;

    // One row per document-id class: ids differing only in the last character, ids
    // agreeing on a long prefix and differing far past 73 characters, ids differing
    // in length, and a short one.
    let long_prefix = "d".repeat(600);
    let document_ids: Vec<String> = vec![
        "doc".to_string(),
        "doc-a".to_string(),
        "doc-b".to_string(),
        format!("{long_prefix}-one"),
        format!("{long_prefix}-two"),
        format!("{long_prefix}-two-more"),
    ];
    for document_id in &document_ids {
        put_text_document(
            &router,
            &index_id,
            document_id,
            "Shared title",
            "every document carries the shared term kendra",
        )
        .await;
    }

    let (status, body) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "kendra", "PageSize": 100 }),
    )
    .await;
    assert_eq!(status, 200, "Query: got {body:?}");
    let items = body["ResultItems"]
        .as_array()
        .unwrap_or_else(|| panic!("no ResultItems: {body:?}"));
    assert_eq!(
        items.len(),
        document_ids.len(),
        "every stored document matches: expected {} results, got {items:?}",
        document_ids.len()
    );

    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for item in items {
        let document_id = item["DocumentId"].as_str().unwrap_or_default();
        let result_id = item["Id"].as_str().unwrap_or_default();
        assert!(
            !result_id.is_empty() && result_id.chars().count() <= 73,
            "case {document_id:?}: ResultId is modelled 1..73 characters, got {result_id:?}"
        );
        if let Some(other) = seen.insert(result_id, document_id) {
            panic!(
                "case {document_id:?}: ResultId {result_id:?} is already the result id of \
                 document {other:?}"
            );
        }
    }
}

/// `ListIndices` pages over every stored index exactly once, whatever `MaxResults` is.
#[tokio::test]
async fn list_indices_pages_every_index_exactly_once() {
    let router = kendra();

    let mut created: Vec<String> = Vec::new();
    for n in 0..5 {
        created.push(create_index(&router, &format!("index-{n}")).await);
    }
    created.sort();

    // Every page size from the modelled 1..100 range that the stored set can show a
    // difference at, plus the no-MaxResults case.
    for max_results in [None, Some(1), Some(2), Some(4), Some(5), Some(100)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        for _ in 0..10 {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["MaxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListIndices", request.clone()).await;
            assert_eq!(status, 200, "ListIndices({request:?}): got {body:?}");
            let items = body["IndexConfigurationSummaryItems"]
                .as_array()
                .unwrap_or_else(|| panic!("ListIndices({request:?}) returned no items: {body:?}"))
                .clone();
            if let Some(max) = max_results {
                assert!(
                    i64::try_from(items.len()).unwrap_or(i64::MAX) <= max,
                    "ListIndices(MaxResults={max}) returned {} items",
                    items.len()
                );
            }
            for item in &items {
                let id = item["Id"].as_str().unwrap_or_default().to_string();
                assert!(
                    !seen.contains(&id),
                    "ListIndices(MaxResults={max_results:?}) repeated index {id}"
                );
                assert!(
                    item["Status"] == json!("CREATING") || item["Status"] == json!("ACTIVE"),
                    "a listed index reports its stored status: got {item:?}"
                );
                seen.push(id);
            }
            next_token = body["NextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen, created,
            "ListIndices(MaxResults={max_results:?}) must page every index exactly once"
        );
    }

    // The token is opaque: it is not an index id handed back as itself.
    let (_, first_page) = call(&router, "ListIndices", json!({ "MaxResults": 1 })).await;
    let token = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a NextToken: {first_page:?}"))
        .to_string();
    assert!(
        !created.contains(&token),
        "NextToken must not be an index id itself, got {token:?}"
    );

    // And a token this operation never issued is refused, naming the member.
    // The rows cover both ways a token can be wrong: it is not decodable at all,
    // and it decodes but carries no token envelope of this operation's.
    for bad_token in [
        "not-a-token!",
        "0",
        &aws_smithy_types::base64::encode("just some text"),
        &aws_smithy_types::base64::encode(created[0].as_str()),
    ] {
        let (status, body) = call(
            &router,
            "ListIndices",
            json!({ "MaxResults": 1, "NextToken": bad_token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "ListIndices(NextToken={bad_token:?}): got {status} {body:?}"
        );
    }
}

/// `BatchPutDocument` stores what it can accept and reports what it cannot, per
/// document, rather than failing or silently accepting the batch.
#[tokio::test]
async fn batch_put_document_reports_each_document_it_cannot_accept() {
    let router = kendra();
    let index_id = create_index(&router, "ingest-index").await;
    wait_until_active(&router, &index_id).await;

    // (document, expected ErrorCode or None when the document is accepted)
    let cases: Vec<(Value, Option<&str>)> = vec![
        (
            json!({ "Id": "ok-text", "Blob": aws_smithy_types::base64::encode("plain text body") }),
            None,
        ),
        (
            json!({
                "Id": "ok-typed",
                "Title": "A title",
                "Blob": aws_smithy_types::base64::encode("another body"),
                "ContentType": "MD",
            }),
            None,
        ),
        // An empty body is content, just no text.
        (
            json!({ "Id": "ok-empty", "Blob": aws_smithy_types::base64::encode("") }),
            None,
        ),
        // Bytes that are not UTF-8 text: no text can be extracted from them.
        (
            json!({ "Id": "bad-binary", "Blob": "/v7+/g==", "ContentType": "PDF" }),
            Some("InvalidRequest"),
        ),
        // Neither Blob nor S3Path: the document carries no content at all.
        (json!({ "Id": "bad-empty" }), Some("InvalidRequest")),
        // Content named by S3Path: the emulator cannot read it.
        (
            json!({ "Id": "bad-s3", "S3Path": { "Bucket": "docs", "Key": "a.txt" } }),
            Some("InternalError"),
        ),
    ];

    let documents: Vec<Value> = cases.iter().map(|(document, _)| document.clone()).collect();
    let (status, body) = call(
        &router,
        "BatchPutDocument",
        json!({ "IndexId": index_id, "Documents": documents }),
    )
    .await;
    assert_eq!(status, 200, "BatchPutDocument: got {body:?}");

    let failures = body["FailedDocuments"]
        .as_array()
        .unwrap_or_else(|| panic!("BatchPutDocument returned no FailedDocuments: {body:?}"))
        .clone();
    for (document, expected_code) in &cases {
        let id = document["Id"].as_str().unwrap_or_default();
        let reported = failures
            .iter()
            .find(|failure| failure["Id"] == json!(id));
        match expected_code {
            None => assert!(
                reported.is_none(),
                "document {id} must be accepted, got {reported:?}"
            ),
            Some(code) => {
                let reported = reported.unwrap_or_else(|| {
                    panic!("document {id} must be reported as failed, got {failures:?}")
                });
                assert_eq!(
                    reported["ErrorCode"],
                    json!(code),
                    "document {id}: expected ErrorCode {code}, got {reported:?}"
                );
                assert!(
                    reported["ErrorMessage"]
                        .as_str()
                        .is_some_and(|message| message.contains(id)),
                    "document {id}: the failure must say which document it is, got {reported:?}"
                );
            }
        }
    }

    // Only the accepted documents are in the index.
    let accepted = cases
        .iter()
        .filter(|(_, expected)| expected.is_none())
        .count();
    let (_, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextDocumentsCount"],
        json!(accepted),
        "only accepted documents are indexed, got {described:?}"
    );

    // A put with an id already in the index replaces that document rather than adding one.
    put_text_document(&router, &index_id, "ok-text", "New title", "a replaced body").await;
    let (_, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextDocumentsCount"],
        json!(accepted),
        "a document id is unique per index, got {described:?}"
    );
    let (_, found) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "replaced" }),
    )
    .await;
    assert_eq!(
        found["ResultItems"][0]["DocumentId"],
        json!("ok-text"),
        "the replacement is what is searched, got {found:?}"
    );
    let (_, gone) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "plain" }),
    )
    .await;
    assert_eq!(
        gone["TotalNumberOfResults"],
        json!(0),
        "the replaced version is no longer searchable, got {gone:?}"
    );

    // One batch naming the same id twice stores ONE document: ids are unique per
    // index, so the later document in the batch is the one that is kept.
    let (status, twice) = call(
        &router,
        "BatchPutDocument",
        json!({
            "IndexId": index_id,
            "Documents": [
                { "Id": "doc-twice", "Blob": aws_smithy_types::base64::encode("first version") },
                { "Id": "doc-twice", "Blob": aws_smithy_types::base64::encode("second version") },
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "BatchPutDocument with a repeated id: got {twice:?}");
    assert_eq!(twice["FailedDocuments"], json!([]), "got {twice:?}");
    let (_, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextDocumentsCount"],
        json!(accepted + 1),
        "a repeated id in one batch is one document, got {described:?}"
    );
    let (_, second) = call(
        &router,
        "Query",
        json!({ "IndexId": index_id, "QueryText": "second" }),
    )
    .await;
    assert_eq!(
        second["ResultItems"][0]["DocumentId"],
        json!("doc-twice"),
        "the later document in the batch is the stored one, got {second:?}"
    );
}

/// Every operation that names an index answers `ResourceNotFoundException` (HTTP 404)
/// for an `IndexId` no stored index matches.
#[tokio::test]
async fn an_index_id_nothing_stored_matches_is_not_found() {
    let router = kendra();

    let cases: Vec<(&str, Value)> = vec![
        ("DescribeIndex", json!({ "Id": ABSENT_INDEX_ID })),
        ("DeleteIndex", json!({ "Id": ABSENT_INDEX_ID })),
        (
            "BatchPutDocument",
            json!({
                "IndexId": ABSENT_INDEX_ID,
                "Documents": [{ "Id": "doc-1", "Blob": aws_smithy_types::base64::encode("body") }],
            }),
        ),
        (
            "Query",
            json!({ "IndexId": ABSENT_INDEX_ID, "QueryText": "anything" }),
        ),
        // The resource is resolved before the request is judged: a query that also
        // asks for something this slice answers 501 for is still a 404.
        (
            "Query",
            json!({
                "IndexId": ABSENT_INDEX_ID,
                "QueryText": "anything",
                "UserContext": { "UserId": "someone" },
            }),
        ),
    ];

    for (operation, request) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "{operation} on an absent index: got {status} {body:?}"
        );
    }
}

/// `CreateIndex` applies the rules the model states: the default edition, the
/// per-client-token idempotency, the unique name, and the Gen AI edition's refusals.
#[tokio::test]
async fn create_index_applies_the_rules_the_model_states() {
    let router = kendra();

    // The model's own doc: "If you don't supply a value, the default is
    // ENTERPRISE_EDITION."
    let (status, created) = call(
        &router,
        "CreateIndex",
        json!({ "Name": "default-edition", "RoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(status, 200, "CreateIndex: got {created:?}");
    let defaulted = created["Id"].as_str().unwrap_or_default().to_string();
    let (_, described) = call(&router, "DescribeIndex", json!({ "Id": defaulted })).await;
    assert_eq!(
        described["Edition"],
        json!("ENTERPRISE_EDITION"),
        "an index created without an Edition defaults to ENTERPRISE_EDITION, got {described:?}"
    );

    // Each modelled edition round-trips.
    for edition in [
        "DEVELOPER_EDITION",
        "ENTERPRISE_EDITION",
        "GEN_AI_ENTERPRISE_EDITION",
    ] {
        let (status, created) = call(
            &router,
            "CreateIndex",
            json!({ "Name": format!("edition-{edition}"), "RoleArn": ROLE_ARN, "Edition": edition }),
        )
        .await;
        assert_eq!(status, 200, "CreateIndex({edition}): got {created:?}");
        let id = created["Id"].as_str().unwrap_or_default().to_string();
        let (_, described) = call(&router, "DescribeIndex", json!({ "Id": id })).await;
        assert_eq!(
            described["Edition"],
            json!(edition),
            "CreateIndex({edition}) must store the edition, got {described:?}"
        );
    }

    // "Multiple calls to the CreateIndex API with the same client token will create
    // only one index."
    let request = json!({
        "Name": "token-index",
        "RoleArn": ROLE_ARN,
        "ClientToken": "a-client-token",
    });
    let (status, first) = call(&router, "CreateIndex", request.clone()).await;
    assert_eq!(status, 200, "CreateIndex: got {first:?}");
    let (status, retried) = call(&router, "CreateIndex", request).await;
    assert_eq!(status, 200, "CreateIndex retry: got {retried:?}");
    assert_eq!(
        retried["Id"], first["Id"],
        "a retry with the same ClientToken must answer with the same index"
    );

    // A second index cannot take a name that is already in use.
    let (status, conflict) = call(
        &router,
        "CreateIndex",
        json!({ "Name": "token-index", "RoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        (status, error_code(&conflict).as_str()),
        (400, "ResourceAlreadyExistException"),
        "a duplicate index name: got {status} {conflict:?}"
    );

    // Three rules the model documents for a Gen AI Enterprise Edition index.
    let gen_ai_refusals: Vec<(&str, Value)> = vec![
        (
            "UserContextPolicy=USER_TOKEN",
            json!({ "UserContextPolicy": "USER_TOKEN" }),
        ),
        (
            "UserTokenConfigurations",
            json!({ "UserTokenConfigurations": [{ "JwtTokenTypeConfiguration": {
                "KeyLocation": "URL",
                "URL": "https://example.com/jwks.json",
            } }] }),
        ),
        (
            "UserGroupResolutionConfiguration",
            json!({ "UserGroupResolutionConfiguration": { "UserGroupResolutionMode": "AWS_SSO" } }),
        ),
    ];
    for (case, extra) in gen_ai_refusals {
        let mut request = json!({
            "Name": format!("gen-ai-{}", case.to_lowercase().replace(['=', '.'], "-")),
            "RoleArn": ROLE_ARN,
            "Edition": "GEN_AI_ENTERPRISE_EDITION",
        });
        for (key, value) in extra.as_object().expect("an object of extra members") {
            request[key] = value.clone();
        }
        let (status, refused) = call(&router, "CreateIndex", request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "GEN_AI_ENTERPRISE_EDITION with {case}: got {status} {refused:?}"
        );
    }

    // An Edition outside the modelled enum is refused, not stored.
    let (status, refused) = call(
        &router,
        "CreateIndex",
        json!({ "Name": "bad-edition", "RoleArn": ROLE_ARN, "Edition": "FREE_EDITION" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "an unmodelled Edition: got {status} {refused:?}"
    );
}

/// A `Query` member whose effect this slice does not compute is answered with a 501,
/// never silently ignored — the answer would otherwise be to a different question.
#[tokio::test]
async fn a_query_asking_for_what_is_not_computed_is_not_implemented() {
    let router = kendra();
    let index_id = create_index(&router, "unsupported-index").await;
    wait_until_active(&router, &index_id).await;
    put_text_document(&router, &index_id, "doc-1", "Title", "a body with kendra in it").await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "AttributeFilter",
            json!({ "AttributeFilter": { "EqualsTo": {
                "Key": "_language_code",
                "Value": { "StringValue": "en" },
            } } }),
        ),
        (
            "Facets",
            json!({ "Facets": [{ "DocumentAttributeKey": "_category", "MaxResults": 1 }] }),
        ),
        (
            "SortingConfiguration",
            json!({ "SortingConfiguration": {
                "DocumentAttributeKey": "_created_at",
                "SortOrder": "DESC",
            } }),
        ),
        (
            "UserContext",
            json!({ "UserContext": { "UserId": "someone" } }),
        ),
        (
            "SpellCorrectionConfiguration",
            json!({ "SpellCorrectionConfiguration": {
                "IncludeQuerySpellCheckSuggestions": true,
            } }),
        ),
        (
            "SortingConfigurations",
            json!({ "SortingConfigurations": [{
                "DocumentAttributeKey": "_created_at",
                "SortOrder": "ASC",
            }] }),
        ),
        (
            "CollapseConfiguration",
            json!({ "CollapseConfiguration": { "DocumentAttributeKey": "_category" } }),
        ),
        (
            "DocumentRelevanceOverrideConfigurations",
            json!({ "DocumentRelevanceOverrideConfigurations": [{
                "Name": "_category",
                "Relevance": { "Importance": 5 },
            }] }),
        ),
    ];

    for (member, extra) in cases {
        let mut request = json!({ "IndexId": index_id, "QueryText": "kendra" });
        for (key, value) in extra.as_object().expect("an object of extra members") {
            request[key] = value.clone();
        }
        let (status, body) = call(&router, "Query", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "Query with {member}: got {status} {body:?}"
        );
    }

    // A query with no searchable criterion at all is a bad request, not a 501.
    for query_text in [None, Some(""), Some("   "), Some("!!!")] {
        let mut request = json!({ "IndexId": index_id });
        if let Some(text) = query_text {
            request["QueryText"] = json!(text);
        }
        let (status, body) = call(&router, "Query", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "Query(QueryText={query_text:?}): got {status} {body:?}"
        );
    }

    // The published quota is 1,000 characters of query text: the bound itself is
    // served, one character past it is refused.
    for (characters, expected) in [(999, 200), (1_000, 200), (1_001, 400)] {
        let query_text = "kendra ".repeat(characters / 7 + 1)[..characters].to_string();
        assert_eq!(query_text.chars().count(), characters, "test row setup");
        let (status, body) = call(
            &router,
            "Query",
            json!({ "IndexId": index_id, "QueryText": query_text }),
        )
        .await;
        assert_eq!(
            status, expected,
            "Query with a {characters}-character QueryText: got {status} {body:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "Query with a {characters}-character QueryText: got {body:?}"
            );
        }
    }
}

/// An index holds at most its edition's published document quota; the request that
/// would take it past that is refused with the modelled quota error.
#[tokio::test]
async fn an_index_takes_at_most_its_editions_document_quota() {
    let router = kendra();
    let index_id = create_index(&router, "quota-index").await;
    wait_until_active(&router, &index_id).await;

    // A DEVELOPER_EDITION index holds 10,000 documents; `Documents` is modelled
    // 1..10 per call, so filling it takes 1,000 calls.
    let quota = 10_000;
    let per_call = 10;
    for batch in 0..(quota / per_call) {
        let documents: Vec<Value> = (0..per_call)
            .map(|n| {
                json!({
                    "Id": format!("doc-{}", batch * per_call + n),
                    "Blob": aws_smithy_types::base64::encode("a short body"),
                })
            })
            .collect();
        let (status, body) = call(
            &router,
            "BatchPutDocument",
            json!({ "IndexId": index_id, "Documents": documents }),
        )
        .await;
        assert_eq!(status, 200, "BatchPutDocument(batch {batch}): got {body:?}");
        assert_eq!(
            body["FailedDocuments"],
            json!([]),
            "BatchPutDocument(batch {batch}): got {body:?}"
        );
    }

    let (_, described) = call(&router, "DescribeIndex", json!({ "Id": index_id })).await;
    assert_eq!(
        described["IndexStatistics"]["TextDocumentStatistics"]["IndexedTextDocumentsCount"],
        json!(quota),
        "the index must hold its whole quota, got {described:?}"
    );

    // One more NEW document is over the quota.
    let (status, refused) = call(
        &router,
        "BatchPutDocument",
        json!({
            "IndexId": index_id,
            "Documents": [{ "Id": "doc-one-too-many", "Blob": aws_smithy_types::base64::encode("x") }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (402, "ServiceQuotaExceededException"),
        "BatchPutDocument past the quota: got {status} {refused:?}"
    );

    // But replacing a document already in the index is not new growth.
    let (status, accepted) = call(
        &router,
        "BatchPutDocument",
        json!({
            "IndexId": index_id,
            "Documents": [{ "Id": "doc-0", "Blob": aws_smithy_types::base64::encode("replaced") }],
        }),
    )
    .await;
    assert_eq!(status, 200, "BatchPutDocument replacing a document: got {accepted:?}");
    assert_eq!(accepted["FailedDocuments"], json!([]), "got {accepted:?}");
}

/// The account holds a bounded number of indices: the create past that bound is
/// refused with the modelled quota error, and succeeds again once one is deleted.
#[tokio::test]
async fn the_account_takes_at_most_its_indices_quota() {
    let router = kendra();

    // `crate::state::MAX_INDICES` indices fit; each one is a distinct name.
    let quota = 100;
    for n in 0..quota {
        let (status, body) = call(
            &router,
            "CreateIndex",
            json!({ "Name": format!("index-{n}"), "RoleArn": ROLE_ARN }),
        )
        .await;
        assert_eq!(status, 200, "CreateIndex(index-{n}): got {body:?}");
    }

    let (status, refused) = call(
        &router,
        "CreateIndex",
        json!({ "Name": "index-one-too-many", "RoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (402, "ServiceQuotaExceededException"),
        "CreateIndex past the indices quota: got {status} {refused:?}"
    );

    // Deleting an index frees its slot — once the removal the clock owes has landed.
    let (_, listed) = call(&router, "ListIndices", json!({ "MaxResults": 1 })).await;
    let victim = listed["IndexConfigurationSummaryItems"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("ListIndices returned no index: {listed:?}"))
        .to_string();
    let (status, deleted) = call(&router, "DeleteIndex", json!({ "Id": victim })).await;
    assert_eq!(status, 200, "DeleteIndex({victim}): got {deleted:?}");

    let mut created_after_delete = None;
    for _ in 0..100 {
        let (status, body) = call(
            &router,
            "CreateIndex",
            json!({ "Name": "index-after-delete", "RoleArn": ROLE_ARN }),
        )
        .await;
        if status == 200 {
            created_after_delete = body["Id"].as_str().map(ToString::to_string);
            break;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (402, "ServiceQuotaExceededException"),
            "while the deleted index still exists the account is full: got {status} {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        created_after_delete.is_some(),
        "once the deleted index is gone its slot must be free again"
    );
}
