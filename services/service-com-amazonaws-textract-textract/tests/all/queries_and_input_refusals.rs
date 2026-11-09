// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Textract's `QUERIES` feature and of the code this service
//! words a boundary refusal with (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: Textract.<Operation>`).
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * a document analysed with the `QUERIES` feature answers with one `QUERY` block per
//!   query, carrying the query AS ASKED, and relates it to a `QUERY_RESULT` only where
//!   the document really holds the answer — over query counts, aliases, page filters and
//!   both the text and the raster read paths;
//! * `QueriesConfig` and the `QUERIES` `FeatureType` are a pair: either without the other
//!   is refused with the operation's input-constraint error, and that check runs BEFORE
//!   any feature dispatch, so it is never hidden behind a "not implemented" answer;
//! * a request refused at the modelled-constraint boundary (an absent required member, a
//!   value outside a declared length, pattern or enum) is answered with the error this
//!   service documents for a constraint violation, `InvalidParameterException`, whichever
//!   operation and member it was — while a value INSIDE the published bounds is served.

use std::collections::BTreeSet;

use image::ImageFormat;
use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;

use crate::document_extraction::{
    block_id, block_type, blocks_of, call, error_code, harness, of_type, text_document,
};
use crate::raster_documents::{raster_document, render, Paper};

/// A document with three labelled fields, used where a case needs a document whose
/// answers are known from its own text.
const INVOICE: &str = "Invoice Number: INV-1024\nTotal Due: 42.50\nVendor Name: Acme Supplies";

/// One query of a case: what is asked, and what the document answers it with.
#[derive(Debug, Clone)]
struct Asked {
    text: &'static str,
    alias: Option<&'static str>,
    pages: Option<Vec<&'static str>>,
    /// The answer the document holds, or `None` where it holds none and the service must
    /// report the query with no `ANSWER` rather than a guess.
    answer: Option<&'static str>,
    /// The page the `QUERY` block reports: the first page the query asks about.
    page: i64,
}

impl Asked {
    fn new(text: &'static str, answer: Option<&'static str>) -> Self {
        Self {
            text,
            alias: None,
            pages: None,
            answer,
            page: 1,
        }
    }

    fn aliased(mut self, alias: &'static str) -> Self {
        self.alias = Some(alias);
        self
    }

    fn on_pages(mut self, pages: &[&'static str], page: i64) -> Self {
        self.pages = Some(pages.to_vec());
        self.page = page;
        self
    }

    fn request(&self) -> Value {
        let mut query = json!({ "Text": self.text });
        if let Some(alias) = self.alias {
            query["Alias"] = json!(alias);
        }
        if let Some(pages) = &self.pages {
            query["Pages"] = json!(pages);
        }
        query
    }
}

/// One row of the QUERIES table.
struct QueryCase {
    name: &'static str,
    document: Value,
    feature_types: Vec<&'static str>,
    queries: Vec<Asked>,
    /// Block types the answer must also carry, for a case that asks for another feature
    /// alongside `QUERIES`.
    also: Vec<&'static str>,
}

fn analyze_request(case: &QueryCase) -> Value {
    json!({
        "Document": case.document,
        "FeatureTypes": case.feature_types,
        "QueriesConfig": { "Queries": case.queries.iter().map(Asked::request).collect::<Vec<_>>() },
    })
}

fn query_of(block: &Value) -> &Value {
    block.get("Query").unwrap_or(&Value::Null)
}

fn member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn related_ids(block: &Value, kind: &str) -> Vec<String> {
    block
        .get("Relationships")
        .and_then(Value::as_array)
        .map(|rels| {
            rels.iter()
                .filter(|rel| rel.get("Type").and_then(Value::as_str) == Some(kind))
                .flat_map(|rel| {
                    rel.get("Ids")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|id| id.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn find<'a>(blocks: &'a [Value], id: &str) -> Option<&'a Value> {
    blocks.iter().find(|block| block_id(block) == id)
}

/// The cases the QUERIES rule is asserted over: they differ in how many queries are
/// asked, whether a query carries an alias, which pages it asks about, whether the
/// document answers it, which other feature is asked for alongside, and whether the
/// document's text was read verbatim or RECOGNISED in a raster's pixels.
fn query_cases() -> Vec<QueryCase> {
    let rendered = render(
        &["Invoice Number: INV-1024", "Total Due: 42.50"],
        Paper::plain(),
    );
    vec![
        QueryCase {
            name: "one query the document answers",
            document: text_document(INVOICE),
            feature_types: vec!["QUERIES"],
            queries: vec![Asked::new(
                "What is the invoice number?",
                Some("INV-1024"),
            )],
            also: Vec::new(),
        },
        QueryCase {
            name: "three queries, one of them unanswerable",
            document: text_document(INVOICE),
            feature_types: vec!["QUERIES"],
            queries: vec![
                Asked::new("What is the invoice number?", Some("INV-1024")).aliased("INVOICE_ID"),
                Asked::new("What is the total due?", Some("42.50")).aliased("TOTAL"),
                Asked::new("What is the shipping address?", None).aliased("SHIP_TO"),
            ],
            also: Vec::new(),
        },
        QueryCase {
            name: "a question whose terms mostly miss the label is left unanswered",
            document: text_document(INVOICE),
            feature_types: vec!["QUERIES"],
            queries: vec![
                Asked::new("What is the vendor name?", Some("Acme Supplies")),
                // Most of this question's terms are about nothing the document labels, so
                // it falls under the retrieval floor and is reported with no answer.
                Asked::new("Which vendor billed us?", None),
                Asked::new("What is the total due?", Some("42.50")).aliased("TOTAL"),
            ],
            also: Vec::new(),
        },
        QueryCase {
            // A document is ONE page: the page a query names decides whether the
            // document's text is in reach of it at all, and page 2 of a one-page document
            // holds nothing.
            name: "a query that names a page the document does not have finds nothing",
            document: text_document(INVOICE),
            feature_types: vec!["QUERIES"],
            queries: vec![
                Asked::new("What is the invoice number?", None).on_pages(&["2"], 1),
                Asked::new("What is the total due?", None).on_pages(&["2-*"], 1),
            ],
            also: Vec::new(),
        },
        QueryCase {
            name: "every page selector that reaches the document's own page",
            document: text_document(INVOICE),
            feature_types: vec!["QUERIES"],
            queries: vec![
                Asked::new("What is the invoice number?", Some("INV-1024")).on_pages(&["*"], 1),
                Asked::new("What is the total due?", Some("42.50")).on_pages(&["1"], 1),
                Asked::new("What is the vendor name?", Some("Acme Supplies"))
                    .on_pages(&["1-2"], 1),
                Asked::new("What is the invoice number?", Some("INV-1024")).on_pages(&["1-*"], 1),
            ],
            also: Vec::new(),
        },
        QueryCase {
            name: "QUERIES alongside FORMS and TABLES",
            document: text_document(
                "Invoice Number: INV-1024\nTotal Due: 42.50\nItem Code\tQty\nBolt Mk2\t4",
            ),
            feature_types: vec!["TABLES", "FORMS", "QUERIES"],
            queries: vec![Asked::new(
                "What is the invoice number?",
                Some("INV-1024"),
            )],
            also: vec!["KEY_VALUE_SET", "TABLE", "CELL"],
        },
        QueryCase {
            name: "a query answered from text recognised in a raster",
            document: raster_document(&rendered, ImageFormat::Png),
            feature_types: vec!["QUERIES"],
            queries: vec![
                Asked::new("What is the invoice number?", Some("INV-1024")),
                Asked::new("What is the shipping address?", None),
            ],
            also: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn queries_report_one_query_block_per_query_answered_from_the_document() {
    let router = harness();

    for case in query_cases() {
        let (status, body) = call(&router, "AnalyzeDocument", &analyze_request(&case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} {body}",
            case.name
        );

        let blocks = blocks_of(&body);
        let queries = of_type(&blocks, "QUERY");
        assert_eq!(
            queries.len(),
            case.queries.len(),
            "case {:?}: expected one QUERY block per query, got {}",
            case.name,
            queries.len()
        );

        for (asked, block) in case.queries.iter().zip(queries.iter()) {
            let reported = query_of(block);
            assert_eq!(
                member(reported, "Text"),
                asked.text,
                "case {:?}: QUERY must carry the query as asked, got {reported}",
                case.name
            );
            match asked.alias {
                Some(alias) => assert_eq!(
                    member(reported, "Alias"),
                    alias,
                    "case {:?}: QUERY must carry the query's alias, got {reported}",
                    case.name
                ),
                None => assert!(
                    reported.get("Alias").is_none(),
                    "case {:?}: a query asked with no alias must report none, got {reported}",
                    case.name
                ),
            }
            assert_eq!(
                block.get("Page").and_then(Value::as_i64),
                Some(asked.page),
                "case {:?}: QUERY for {:?} must report the first page it asks about, got {block}",
                case.name,
                asked.text
            );
            // A QUERY is a question asked of the page, not a region of it.
            assert!(
                block.get("Geometry").is_none(),
                "case {:?}: QUERY for {:?} must carry no Geometry, got {block}",
                case.name,
                asked.text
            );

            let answers = related_ids(block, "ANSWER");
            match asked.answer {
                Some(expected) => {
                    assert_eq!(
                        answers.len(),
                        1,
                        "case {:?}: query {:?} is answered by the document, so it must relate \
                         to exactly one answer, got {answers:?}",
                        case.name,
                        asked.text
                    );
                    let result = find(&blocks, &answers[0]).unwrap_or_else(|| {
                        panic!(
                            "case {:?}: the ANSWER of {:?} must be a block of the answer, \
                             {:?} resolves to none",
                            case.name, asked.text, answers[0]
                        )
                    });
                    assert_eq!(
                        block_type(result),
                        "QUERY_RESULT",
                        "case {:?}: the ANSWER of {:?} must be a QUERY_RESULT, got {result}",
                        case.name,
                        asked.text
                    );
                    assert_eq!(
                        member(result, "Text"),
                        expected,
                        "case {:?}: the answer to {:?} must be the value the document holds, \
                         got {result}",
                        case.name,
                        asked.text
                    );
                    let confidence = result
                        .get("Confidence")
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                    assert!(
                        confidence > 0.0 && confidence <= 100.0,
                        "case {:?}: the answer to {:?} must report a Percent confidence, \
                         got {confidence}",
                        case.name,
                        asked.text
                    );
                }
                None => {
                    assert!(
                        answers.is_empty(),
                        "case {:?}: the document answers nothing for {:?}, so the QUERY must \
                         carry no ANSWER, got {answers:?}",
                        case.name,
                        asked.text
                    );
                }
            }
        }

        // A query the document does not answer adds no QUERY_RESULT at all.
        let expected_results = case
            .queries
            .iter()
            .filter(|asked| asked.answer.is_some())
            .count();
        assert_eq!(
            of_type(&blocks, "QUERY_RESULT").len(),
            expected_results,
            "case {:?}: expected {expected_results} QUERY_RESULT blocks, got {}",
            case.name,
            of_type(&blocks, "QUERY_RESULT").len()
        );

        for wanted in &case.also {
            assert!(
                !of_type(&blocks, wanted).is_empty(),
                "case {:?}: FeatureTypes {:?} must still derive {wanted} blocks, got {:?}",
                case.name,
                case.feature_types,
                blocks.iter().map(block_type).collect::<BTreeSet<_>>()
            );
        }
    }
}

#[tokio::test]
async fn the_same_queries_of_the_same_document_name_the_same_blocks() {
    let router = harness();
    let case = QueryCase {
        name: "repeated analysis",
        document: text_document(INVOICE),
        feature_types: vec!["QUERIES"],
        queries: vec![
            Asked::new("What is the invoice number?", Some("INV-1024")),
            Asked::new("What is the total due?", Some("42.50")),
        ],
        also: Vec::new(),
    };
    let request = analyze_request(&case);

    let (_, first) = call(&router, "AnalyzeDocument", &request).await;
    let (_, second) = call(&router, "AnalyzeDocument", &request).await;

    let ids = |body: &Value| -> Vec<String> {
        blocks_of(body)
            .iter()
            .filter(|block| matches!(block_type(block), "QUERY" | "QUERY_RESULT"))
            .map(|block| format!("{}:{}", block_type(block), block_id(block)))
            .collect()
    };
    assert_eq!(
        ids(&first),
        ids(&second),
        "a document's query blocks are derived from its identity, so two reads must name \
         the same blocks: {:?} then {:?}",
        ids(&first),
        ids(&second)
    );
}

/// One row of the pairing / quota table: a request and the operation it is sent to.
struct RefusalCase {
    name: &'static str,
    operation: &'static str,
    request: Value,
}

/// The S3 location the asynchronous rows submit.
///
/// `pub(crate)`: the refusal-wording suite starts its jobs from this same location.
pub(crate) fn s3_location() -> Value {
    json!({ "S3Object": { "Bucket": "documents", "Name": "invoice.pdf" } })
}

fn queries(count: usize) -> Value {
    json!({
        "Queries": (0..count)
            .map(|index| json!({ "Text": format!("What is field {index}?") }))
            .collect::<Vec<_>>()
    })
}

#[tokio::test]
async fn queries_config_and_the_queries_feature_must_be_supplied_together() {
    let router = harness();

    let cases = vec![
        RefusalCase {
            name: "QueriesConfig with FORMS only",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["FORMS"],
                "QueriesConfig": queries(1),
            }),
        },
        RefusalCase {
            name: "QueriesConfig with TABLES only",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["TABLES"],
                "QueriesConfig": queries(2),
            }),
        },
        // The pairing is a bad request whatever else FeatureTypes holds: the 501 for a
        // modelled feature this emulator derives no blocks for must not stand in front
        // of it.
        RefusalCase {
            name: "QueriesConfig beside a feature this emulator derives no blocks for",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["SIGNATURES"],
                "QueriesConfig": queries(1),
            }),
        },
        RefusalCase {
            name: "QUERIES asked for with no QueriesConfig",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
            }),
        },
        // One past the published synchronous quota.
        RefusalCase {
            name: "more queries than one synchronous request may ask",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": queries(state::MAX_QUERIES_PER_REQUEST + 1),
            }),
        },
        RefusalCase {
            name: "Pages uses * beside another entry",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{ "Text": "What is the total due?", "Pages": ["*", "2"] }] },
            }),
        },
        RefusalCase {
            name: "Pages names page 0",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{ "Text": "What is the total due?", "Pages": ["0"] }] },
            }),
        },
        RefusalCase {
            name: "Pages names a descending interval",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{ "Text": "What is the total due?", "Pages": ["3-1"] }] },
            }),
        },
        RefusalCase {
            name: "Pages holds more entries than one query may name",
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{
                    "Text": "What is the total due?",
                    "Pages": (1..=state::MAX_PAGES_PER_QUERY + 1)
                        .map(|page| page.to_string())
                        .collect::<Vec<_>>(),
                }] },
            }),
        },
        // The same pairing rule on the ASYNCHRONOUS analysis, which takes the same two
        // members.
        RefusalCase {
            name: "asynchronous analysis: QueriesConfig with FORMS only",
            operation: "StartDocumentAnalysis",
            request: json!({
                "DocumentLocation": s3_location(),
                "FeatureTypes": ["FORMS"],
                "QueriesConfig": queries(1),
            }),
        },
        RefusalCase {
            name: "asynchronous analysis: QUERIES with no QueriesConfig",
            operation: "StartDocumentAnalysis",
            request: json!({
                "DocumentLocation": s3_location(),
                "FeatureTypes": ["QUERIES"],
            }),
        },
        RefusalCase {
            name: "asynchronous analysis: more queries than one request may ask",
            operation: "StartDocumentAnalysis",
            request: json!({
                "DocumentLocation": s3_location(),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": queries(state::MAX_QUERIES_PER_ASYNC_REQUEST + 1),
            }),
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {:?}: expected InvalidParameterException, got {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_request_at_the_published_query_quota_is_served() {
    let router = harness();

    // The synchronous quota exactly: served, with one QUERY block per query.
    let (status, body) = call(
        &router,
        "AnalyzeDocument",
        &json!({
            "Document": text_document(INVOICE),
            "FeatureTypes": ["QUERIES"],
            "QueriesConfig": queries(state::MAX_QUERIES_PER_REQUEST),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the synchronous quota of {} queries must be served, got {status} {body}",
        state::MAX_QUERIES_PER_REQUEST
    );
    assert_eq!(
        of_type(&blocks_of(&body), "QUERY").len(),
        state::MAX_QUERIES_PER_REQUEST,
        "expected one QUERY block per query at the quota, got {body}"
    );

    // The asynchronous quota is the higher one AWS publishes, so a request at it starts
    // a job rather than being refused.
    let (status, body) = call(
        &router,
        "StartDocumentAnalysis",
        &json!({
            "DocumentLocation": s3_location(),
            "FeatureTypes": ["QUERIES"],
            "QueriesConfig": queries(state::MAX_QUERIES_PER_ASYNC_REQUEST),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the asynchronous quota of {} queries must be served, got {status} {body}",
        state::MAX_QUERIES_PER_ASYNC_REQUEST
    );
    assert!(
        body.get("JobId").and_then(Value::as_str).is_some(),
        "a started analysis must report its JobId, got {body}"
    );
}

/// One row of the boundary-refusal table: the request, and whether the value is inside
/// the member's published bounds.
struct BoundaryCase {
    name: &'static str,
    operation: &'static str,
    request: Value,
    served: bool,
}

/// A name of `length` characters, inside `AdapterName`'s `^[a-zA-Z0-9-_]+$` pattern
/// (smithy_query member CreateAdapterRequest$AdapterName: @length 1..128, that pattern).
fn name_of_length(length: usize) -> String {
    "a".repeat(length)
}

#[tokio::test]
async fn a_constraint_violation_is_refused_as_an_invalid_parameter() {
    let router = harness();

    // Member constraints from smithy_query: CreateAdapterRequest$AdapterName @length
    // 1..128 @pattern ^[a-zA-Z0-9-_]+$, $Description @length 1..256, $FeatureTypes
    // required, $AutoUpdate the AutoUpdate enum; UpdateAdapterRequest$AdapterId required.
    let cases = vec![
        BoundaryCase {
            name: "AdapterName outside the pattern",
            operation: "CreateAdapter",
            request: json!({
                "AdapterName": "sim2real adapter 7!!invalid!!",
                "FeatureTypes": ["QUERIES"],
            }),
            served: false,
        },
        BoundaryCase {
            name: "AdapterName under the minimum length",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "", "FeatureTypes": ["QUERIES"] }),
            served: false,
        },
        BoundaryCase {
            name: "AdapterName one past the maximum length",
            operation: "CreateAdapter",
            request: json!({
                "AdapterName": name_of_length(129),
                "FeatureTypes": ["QUERIES"],
            }),
            served: false,
        },
        BoundaryCase {
            name: "AdapterName at the minimum length",
            operation: "CreateAdapter",
            request: json!({
                "AdapterName": name_of_length(1),
                "FeatureTypes": ["QUERIES"],
            }),
            served: true,
        },
        BoundaryCase {
            name: "AdapterName at the maximum length",
            operation: "CreateAdapter",
            request: json!({
                "AdapterName": name_of_length(128),
                "FeatureTypes": ["QUERIES"],
            }),
            served: true,
        },
        BoundaryCase {
            name: "Description under the minimum length",
            operation: "CreateAdapter",
            request: json!({
                "AdapterName": "described-adapter",
                "Description": "",
                "FeatureTypes": ["QUERIES"],
            }),
            served: false,
        },
        BoundaryCase {
            name: "a required member absent",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "no-features" }),
            served: false,
        },
        BoundaryCase {
            name: "a value outside the AutoUpdate enum",
            operation: "CreateAdapter",
            request: json!({
                "AdapterName": "bad-auto-update",
                "FeatureTypes": ["QUERIES"],
                "AutoUpdate": "SOMETIMES",
            }),
            served: false,
        },
        BoundaryCase {
            name: "another operation's name outside the pattern",
            operation: "UpdateAdapter",
            request: json!({
                "AdapterId": name_of_length(12),
                "AdapterName": "renamed adapter!!",
            }),
            served: false,
        },
        BoundaryCase {
            name: "another operation missing its required id",
            operation: "GetAdapter",
            request: json!({}),
            served: false,
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.operation, &case.request).await;
        if case.served {
            assert_eq!(
                status, 200,
                "case {:?}: a value inside the published bounds must be served, got \
                 {status} {body}",
                case.name
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {:?}: expected InvalidParameterException, got {body}",
            case.name
        );
    }
}
