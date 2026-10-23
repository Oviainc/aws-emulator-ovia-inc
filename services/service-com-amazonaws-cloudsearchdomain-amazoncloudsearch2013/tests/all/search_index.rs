// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudSearch Domain data plane: a document batch is
//! uploaded, and every later `Search` and `Suggest` answer is computed from the
//! documents the index actually holds.
//!
//! Every suite drives the service through the router the emulator serves, so the
//! whole walk — routing, query-parameter binding, the handler, and the restJson1
//! envelope — is exercised. The tables span the class each rule generalises over
//! (both batch formats, all four query parsers, the paging, sorting, faceting,
//! statistics and highlighting options, and the refusals), with the issue's own
//! case as one row among them.

use bytes::Bytes;
use service_com_amazonaws_cloudsearchdomain_amazoncloudsearch2013::state::query::{
    MAX_FACET_SIZE, MAX_HIGHLIGHT_PHRASES, MAX_HIGHLIGHT_TAG_BYTES, MAX_PAGE_DEPTH,
    MAX_QUERY_BYTES, MAX_QUERY_DEPTH, MAX_QUERY_FIELDS, MAX_QUERY_TERMS,
};
use service_com_amazonaws_cloudsearchdomain_amazoncloudsearch2013::state::sdf::{
    MAX_BATCH_BYTES, MAX_DOCUMENT_BYTES,
};
use service_com_amazonaws_cloudsearchdomain_amazoncloudsearch2013::state::{
    MAX_DOCUMENTS_PER_DOMAIN, MAX_DOMAINS,
};
use service_com_amazonaws_cloudsearchdomain_amazoncloudsearch2013::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The document endpoint of the `movies` domain: `UploadDocuments` is submitted here.
const DOC_HOST: &str = "doc-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com";
/// The search endpoint of the SAME domain: `Search`/`Suggest` are submitted here.
const SEARCH_HOST: &str = "search-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com";
/// The search endpoint of a DIFFERENT domain.
const OTHER_HOST: &str = "search-books-9z8y7x.us-east-1.cloudsearchdomain.amazonaws.com";

/// A SigV4 header whose credential scope names this service's signing name, which
/// is how the router resolves which service a request addresses.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudsearch/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The batch the search suites index, in the JSON form of SDF.
const MOVIES_JSON: &str = r#"[
  {"type":"add","id":"doc1","fields":{"title":"Hamlet","genre":["drama","tragedy"],
    "year":1996,"director":"Branagh","runtime":242}},
  {"type":"add","id":"doc2","fields":{"title":"The Comedy of Errors","genre":["comedy"],
    "year":1987,"director":"Moshinsky"}},
  {"type":"add","id":"doc3","fields":{"title":"Solaris","genre":["science fiction","drama"],
    "year":1972,"director":"Tarkovsky"}}
]"#;

/// A router serving this service over one shared engine, with request
/// authentication off so a test does not have to sign.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Percent-encode one query-parameter value, leaving RFC 3986's unreserved set
/// alone. Written here rather than pulled from a crate so this suite's test
/// harness adds no dependency (the configuration service's suite carries the same
/// helper for its form bodies).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Send `request` and return its status and parsed JSON body.
async fn send(
    router: &MultiServiceRouter,
    request: http::Request<Bytes>,
) -> (u16, serde_json::Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.body();
    let parsed = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(body).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(body)
            )
        })
    };
    (status, parsed)
}

/// `UploadDocuments` against `host`.
async fn upload_to(
    router: &MultiServiceRouter,
    host: &str,
    content_type: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/2013-01-01/documents/batch?format=sdk"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .header("content-type", content_type)
        .body(Bytes::from(body.to_string()))
        .expect("build the UploadDocuments request");
    send(router, request).await
}

/// `UploadDocuments` against the `movies` document endpoint.
async fn upload(
    router: &MultiServiceRouter,
    content_type: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    upload_to(router, DOC_HOST, content_type, body).await
}

/// A `GET` against one of this service's paths on `host`, with `params` added to
/// the static query string the model binds.
async fn get(
    router: &MultiServiceRouter,
    host: &str,
    path: &str,
    params: &[(&str, &str)],
) -> (u16, serde_json::Value) {
    let mut query = String::from("format=sdk&pretty=true");
    for (name, value) in params {
        query.push('&');
        query.push_str(name);
        query.push('=');
        query.push_str(&encode(value));
    }
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}{path}?{query}"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("build the request");
    send(router, request).await
}

/// `Search` against the `movies` search endpoint.
async fn search(router: &MultiServiceRouter, params: &[(&str, &str)]) -> (u16, serde_json::Value) {
    get(router, SEARCH_HOST, "/2013-01-01/search", params).await
}

/// `Suggest` against the `movies` search endpoint.
async fn suggest(router: &MultiServiceRouter, params: &[(&str, &str)]) -> (u16, serde_json::Value) {
    get(router, SEARCH_HOST, "/2013-01-01/suggest", params).await
}

/// Index [`MOVIES_JSON`] and check the upload reported what it applied.
async fn seed(router: &MultiServiceRouter) {
    let (status, body) = upload(router, "application/json", MOVIES_JSON).await;
    assert_eq!(status, 200, "seeding the index: body {body}");
    assert_eq!(
        (body["adds"].as_i64(), body["deletes"].as_i64()),
        (Some(3), Some(0)),
        "seeding the index must report 3 adds and 0 deletes, got {body}"
    );
}

/// The ids of a search response's hits, in response order.
fn hit_ids(body: &serde_json::Value) -> Vec<String> {
    body["hits"]["hit"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .map(|hit| hit["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The wire error code of a failed response.
fn error_code(body: &serde_json::Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// A batch row: case name, content type, batch, expected adds and deletes, and
/// the document ids the index holds afterwards.
type BatchCase<'a> = (&'a str, &'a str, &'a str, i64, i64, &'a [&'a str]);
/// A query row: case name, query parameters, and the hit ids expected in order.
type QueryCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);
/// A bounds row: case name, query parameters, and the number of matches the
/// request is expected to answer with.
type BoundsCase<'a> = (&'a str, &'a [(&'a str, &'a str)], i64);
/// A sort row: case name, the value each document stores under the sorted field
/// (`None` for a document that does not have it), and the ids in ascending order.
type SortCase<'a> = (&'a str, &'a [Option<&'a str>], &'a [&'a str]);
/// A facet row: case name, the `facet` parameter, and the buckets expected in order.
type FacetCase<'a> = (&'a str, &'a str, &'a [(&'a str, i64)]);
/// A `return` row: case name, the `return` parameter, and the field names a hit
/// is expected to carry (`None` for no `fields` member at all).
type ReturnCase<'a> = (&'a str, Option<&'a str>, Option<&'a [&'a str]>);
/// A suggest row: case name, suggester, query, size, the (suggestion, id) pairs
/// expected in order, and the expected `found`.
type SuggestCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a [(&'a str, &'a str)],
    i64,
);

#[tokio::test]
async fn a_batch_of_the_served_format_is_applied_and_the_other_is_refused() {
    // The modelled `ContentType` enum declares two batch formats. The JSON form
    // is applied and counted; the XML form is REFUSED with the modelled error
    // (see `state::sdf::XML_CONTENT_TYPE` for why this endpoint does not read
    // it), and a refused batch applies nothing.
    let cases: &[BatchCase<'_>] = &[
        (
            "json adds",
            "application/json",
            MOVIES_JSON,
            3,
            0,
            &["doc1", "doc2", "doc3"],
        ),
        (
            "json add then delete of the same document",
            "application/json",
            r#"[{"type":"add","id":"keep","fields":{"title":"kept"}},
                {"type":"add","id":"gone","fields":{"title":"removed"}},
                {"type":"delete","id":"gone"}]"#,
            2,
            1,
            &["keep"],
        ),
        (
            "json delete of a document that was never there",
            "application/json",
            r#"[{"type":"delete","id":"absent"}]"#,
            0,
            1,
            &[],
        ),
        (
            "json content type with parameters",
            "application/json; charset=utf-8",
            r#"[{"type":"add","id":"doc1","fields":{"title":"Hamlet"}}]"#,
            1,
            0,
            &["doc1"],
        ),
    ];

    for (case, content_type, batch, adds, deletes, indexed) in cases {
        let router = router();
        let (status, body) = upload(&router, content_type, batch).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["status"].as_str(),
            Some("success"),
            "case {case:?}: expected status success, got {body}"
        );
        assert_eq!(
            body["adds"].as_i64(),
            Some(*adds),
            "case {case:?}: expected adds {adds}, got {body}"
        );
        assert_eq!(
            body["deletes"].as_i64(),
            Some(*deletes),
            "case {case:?}: expected deletes {deletes}, got {body}"
        );

        // What the batch applied is what a later search finds.
        let (status, found) = search(
            &router,
            &[("q", "matchall"), ("q.parser", "structured"), ("size", "50")],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: search after upload: {found}");
        assert_eq!(
            hit_ids(&found),
            *indexed,
            "case {case:?}: expected the index to hold {indexed:?}, got {found}"
        );
        assert_eq!(
            found["hits"]["found"].as_i64(),
            Some(indexed.len() as i64),
            "case {case:?}: expected found {}, got {found}",
            indexed.len()
        );
    }

    // The OTHER modelled content type: refused with the modelled error whatever
    // the body carries, and nothing is applied — never a 200 reporting a batch
    // this endpoint did not read. Spans the class: a well-formed XML batch, each
    // entry kind, the content type with parameters, and a body that is not XML.
    let refused_xml: &[(&str, &str, &str)] = &[
        (
            "a well-formed xml add batch",
            "application/xml",
            "<batch><add id=\"doc1\"><field name=\"title\">Hamlet</field></add></batch>",
        ),
        (
            "an xml self-closing delete",
            "application/xml",
            "<batch><delete id=\"doc1\"/></batch>",
        ),
        (
            "an xml content type with parameters",
            "application/xml; charset=utf-8",
            "<batch><add id=\"doc1\"><field name=\"title\">Hamlet</field></add></batch>",
        ),
        ("a body that is not xml at all", "application/xml", "{}"),
    ];
    for (case, content_type, batch) in refused_xml {
        let router = router();
        let (status, body) = upload(&router, content_type, batch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled refusal's HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DocumentServiceException",
            "case {case:?}: expected the modelled error, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("application/json"),
            "case {case:?}: the refusal must name the form this endpoint serves, got {body}"
        );
        // Nothing was applied: the index is still empty.
        let (status, found) = search(
            &router,
            &[("q", "matchall"), ("q.parser", "structured"), ("size", "50")],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: search after the refusal: {found}");
        assert_eq!(
            found["hits"]["found"].as_i64(),
            Some(0),
            "case {case:?}: a refused batch must apply nothing, got {found}"
        );
    }
}

#[tokio::test]
async fn every_query_parser_matches_the_uploaded_documents() {
    let router = router();
    seed(&router).await;

    // One row per query form each of the four modelled parsers accepts.
    let cases: &[QueryCase<'_>] = &[
        ("simple term", &[("q", "hamlet")], &["doc1"]),
        (
            "simple term in a multi-valued field",
            &[("q", "drama")],
            &["doc1", "doc3"],
        ),
        (
            "simple term is case-insensitive",
            &[("q", "HAMLET")],
            &["doc1"],
        ),
        ("simple phrase", &[("q", "\"science fiction\"")], &["doc3"]),
        ("simple prefix", &[("q", "ham*")], &["doc1"]),
        (
            "simple default operator is and",
            &[("q", "drama tragedy")],
            &["doc1"],
        ),
        (
            "simple or via the | operator",
            &[("q", "|comedy |solaris")],
            &["doc2", "doc3"],
        ),
        (
            "simple default operator from q.options",
            &[("q", "comedy solaris"), ("q.options", r#"{"defaultOperator":"or"}"#)],
            &["doc2", "doc3"],
        ),
        (
            "simple fields restriction from q.options",
            &[("q", "drama"), ("q.options", r#"{"fields":["title^2"]}"#)],
            &[],
        ),
        (
            "simple prohibited term",
            &[("q", "drama -hamlet")],
            &["doc3"],
        ),
        ("dismax term", &[("q", "hamlet"), ("q.parser", "dismax")], &["doc1"]),
        (
            "structured matchall",
            &[("q", "matchall"), ("q.parser", "structured")],
            &["doc1", "doc2", "doc3"],
        ),
        (
            "structured field shorthand",
            &[("q", "genre:'drama'"), ("q.parser", "structured")],
            &["doc1", "doc3"],
        ),
        (
            "structured term operator",
            &[("q", "(term field=title 'hamlet')"), ("q.parser", "structured")],
            &["doc1"],
        ),
        (
            "structured prefix operator",
            &[("q", "(prefix field=director 'tar')"), ("q.parser", "structured")],
            &["doc3"],
        ),
        (
            "structured phrase operator",
            &[
                ("q", "(phrase field=genre 'science fiction')"),
                ("q.parser", "structured"),
            ],
            &["doc3"],
        ),
        (
            "structured and",
            &[("q", "(and genre:'drama' year:'1996')"), ("q.parser", "structured")],
            &["doc1"],
        ),
        (
            "structured or",
            &[
                ("q", "(or title:'solaris' genre:'comedy')"),
                ("q.parser", "structured"),
            ],
            &["doc2", "doc3"],
        ),
        (
            "structured not",
            &[("q", "(not genre:'drama')"), ("q.parser", "structured")],
            &["doc2"],
        ),
        (
            "structured term with a boost option",
            &[
                ("q", "(term field=title boost=2 'solaris')"),
                ("q.parser", "structured"),
            ],
            &["doc3"],
        ),
        (
            "lucene fielded term",
            &[("q", "title:hamlet"), ("q.parser", "lucene")],
            &["doc1"],
        ),
        (
            "lucene and",
            &[("q", "genre:drama AND year:1996"), ("q.parser", "lucene")],
            &["doc1"],
        ),
        (
            "lucene or",
            &[
                ("q", "title:solaris OR genre:comedy"),
                ("q.parser", "lucene"),
            ],
            &["doc2", "doc3"],
        ),
        (
            "lucene not",
            &[("q", "genre:drama NOT title:hamlet"), ("q.parser", "lucene")],
            &["doc3"],
        ),
        // AND binds tighter than OR, so this is `(drama AND hamlet) OR comedy` —
        // doc3 (drama, no hamlet) must NOT match, which it would if the terms
        // were flattened into one OR.
        (
            "lucene mixed operators group AND tighter than OR",
            &[
                ("q", "genre:drama AND title:hamlet OR genre:comedy"),
                ("q.parser", "lucene"),
                ("sort", "_id asc"),
            ],
            &["doc1", "doc2"],
        ),
        (
            "simple mixed operators group AND tighter than OR",
            &[
                ("q", "drama hamlet |comedy"),
                ("sort", "_id asc"),
            ],
            &["doc1", "doc2"],
        ),
        (
            "simple mixed operators under an or default operator",
            &[
                ("q", "drama AND hamlet comedy"),
                ("q.options", r#"{"defaultOperator":"or"}"#),
                ("sort", "_id asc"),
            ],
            &["doc1", "doc2"],
        ),
        (
            "a term no document holds matches nothing",
            &[("q", "nosuchterm")],
            &[],
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = search(&router, params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            hit_ids(&body),
            *expected,
            "case {case:?}: expected hits {expected:?}, got {body}"
        );
        assert_eq!(
            body["hits"]["found"].as_i64(),
            Some(expected.len() as i64),
            "case {case:?}: expected found {}, got {body}",
            expected.len()
        );
    }
}

#[tokio::test]
async fn found_counts_the_whole_result_set_while_start_and_size_select_the_page() {
    let router = router();
    seed(&router).await;

    // `found` is the size of the result set, never of the page.
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("first page of one", "0", "1", &["doc1"]),
        ("second page of one", "1", "1", &["doc2"]),
        ("page of two from offset one", "1", "2", &["doc2", "doc3"]),
        ("page larger than the result set", "0", "50", &["doc1", "doc2", "doc3"]),
        ("offset past the result set", "9", "5", &[]),
        (
            "the modelled default size of 0 pages with the documented default of 10",
            "0",
            "0",
            &["doc1", "doc2", "doc3"],
        ),
    ];

    for (case, start, size, expected) in cases {
        let (status, body) = search(
            &router,
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("start", start),
                ("size", size),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["hits"]["found"].as_i64(),
            Some(3),
            "case {case:?}: expected found 3, got {body}"
        );
        assert_eq!(
            hit_ids(&body),
            *expected,
            "case {case:?}: expected page {expected:?}, got {body}"
        );
        assert_eq!(
            body["hits"]["start"].as_i64(),
            Some(start.parse().expect("the case's start is a number")),
            "case {case:?}: expected the page's start echoed, got {body}"
        );
    }
}

#[tokio::test]
async fn a_cursor_pages_the_result_set_from_one_page_to_the_next() {
    let router = router();
    seed(&router).await;

    let (status, first) = search(
        &router,
        &[
            ("q", "matchall"),
            ("q.parser", "structured"),
            ("cursor", "initial"),
            ("size", "2"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the first cursor page: {first}");
    assert_eq!(
        hit_ids(&first),
        ["doc1", "doc2"],
        "the first cursor page must hold the first two hits, got {first}"
    );
    let cursor = first["hits"]["cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a cursor-paged search must return a cursor, got {first}"))
        .to_string();

    let (status, second) = search(
        &router,
        &[
            ("q", "matchall"),
            ("q.parser", "structured"),
            ("cursor", &cursor),
            ("size", "2"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the second cursor page: {second}");
    assert_eq!(
        hit_ids(&second),
        ["doc3"],
        "the second cursor page must resume after the first, got {second}"
    );
    assert_eq!(
        second["hits"]["found"].as_i64(),
        Some(3),
        "every cursor page reports the whole result set, got {second}"
    );
    // THE TERMINATOR IS AN EMPTY PAGE, not a changed token: past the last hit the
    // response repeats the cursor it was given (there is nothing further to
    // resume at), so a client walks until `hit[]` comes back empty.
    let exhausted = second["hits"]["cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a cursor-paged search must return a cursor, got {second}"))
        .to_string();
    let (status, past_the_end) = search(
        &router,
        &[
            ("q", "matchall"),
            ("q.parser", "structured"),
            ("cursor", &exhausted),
            ("size", "2"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the page past the last hit: {past_the_end}");
    assert!(
        hit_ids(&past_the_end).is_empty(),
        "case \"a cursor page past the last hit\": expected no hits, got {past_the_end}"
    );
    assert_eq!(
        past_the_end["hits"]["found"].as_i64(),
        Some(3),
        "case \"a cursor page past the last hit\": the result set is still reported \
         whole, got {past_the_end}"
    );
    // `hits.start` is the offset the REQUEST named, so a cursor-paged response
    // reports 0 on every page — a cursor's resume point is the token's own
    // meaning, and the token that resumes after the page is `hits.cursor` —
    // while a `start`/`size`-paged response echoes the offset it was asked for.
    for (case, body) in [("the first cursor page", &first), ("the second", &second)] {
        assert_eq!(
            body["hits"]["start"].as_i64(),
            Some(0),
            "case {case:?}: a cursor-paged response reports no hit offset, got {body}"
        );
    }
    let (_, paged) = search(
        &router,
        &[
            ("q", "matchall"),
            ("q.parser", "structured"),
            ("start", "1"),
            ("size", "2"),
        ],
    )
    .await;
    assert_eq!(
        paged["hits"]["start"].as_i64(),
        Some(1),
        "case \"a start/size-paged response\": expected the requested offset \
         echoed, got {paged}"
    );
}

#[tokio::test]
async fn facets_are_counted_from_the_matching_documents() {
    let router = router();
    seed(&router).await;

    // Each row is one documented faceting option over the same result set.
    let cases: &[FacetCase<'_>] = &[
        (
            "default order is highest count first",
            r#"{"genre":{}}"#,
            &[
                ("drama", 2),
                ("comedy", 1),
                ("science fiction", 1),
                ("tragedy", 1),
            ],
        ),
        (
            "sort by bucket orders by value",
            r#"{"genre":{"sort":"bucket"}}"#,
            &[
                ("comedy", 1),
                ("drama", 2),
                ("science fiction", 1),
                ("tragedy", 1),
            ],
        ),
        (
            "size truncates the bucket list",
            r#"{"genre":{"size":1}}"#,
            &[("drama", 2)],
        ),
        (
            "explicit buckets are counted in request order",
            r#"{"genre":{"buckets":["comedy","nosuchgenre"]}}"#,
            &[("comedy", 1), ("nosuchgenre", 0)],
        ),
        (
            // `size` caps the buckets the SERVICE chooses; a request that names
            // its buckets gets every one it named, counted, rather than a short
            // list with a success status. Twelve named buckets is past the
            // documented default size of 10.
            "every named bucket is returned, past the default size",
            r#"{"genre":{"buckets":["drama","tragedy","comedy","science fiction","b5","b6","b7","b8","b9","b10","b11","b12"]}}"#,
            &[
                ("drama", 2),
                ("tragedy", 1),
                ("comedy", 1),
                ("science fiction", 1),
                ("b5", 0),
                ("b6", 0),
                ("b7", 0),
                ("b8", 0),
                ("b9", 0),
                ("b10", 0),
                ("b11", 0),
                ("b12", 0),
            ],
        ),
        (
            "a field no document holds has no buckets",
            r#"{"nosuchfield":{}}"#,
            &[],
        ),
    ];

    for (case, facet, expected) in cases {
        let (status, body) = search(
            &router,
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("facet", facet),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        let field = if facet.contains("nosuchfield") {
            "nosuchfield"
        } else {
            "genre"
        };
        let buckets: Vec<(String, i64)> = body["facets"][field]["buckets"]
            .as_array()
            .map(|buckets| {
                buckets
                    .iter()
                    .map(|bucket| {
                        (
                            bucket["value"].as_str().unwrap_or_default().to_string(),
                            bucket["count"].as_i64().unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<(String, i64)> = expected
            .iter()
            .map(|(value, count)| ((*value).to_string(), *count))
            .collect();
        assert_eq!(
            buckets, expected,
            "case {case:?}: expected buckets {expected:?}, got {body}"
        );
    }

    // A facet over a narrowed result set counts only what matched.
    let (status, body) = search(
        &router,
        &[
            ("q", "genre:'drama'"),
            ("q.parser", "structured"),
            ("facet", r#"{"genre":{"sort":"bucket"}}"#),
        ],
    )
    .await;
    assert_eq!(status, 200, "faceting a narrowed result set: {body}");
    let buckets = body["facets"]["genre"]["buckets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        buckets.len(),
        3,
        "case \"facet over a filtered result set\": expected the 3 genres of the two \
         matching documents, got {body}"
    );
}

#[tokio::test]
async fn statistics_are_computed_from_the_matching_values() {
    let router = router();
    seed(&router).await;

    let (status, body) = search(
        &router,
        &[
            ("q", "matchall"),
            ("q.parser", "structured"),
            ("stats", r#"{"year":{},"director":{},"runtime":{}}"#),
        ],
    )
    .await;
    assert_eq!(status, 200, "statistics: expected 200, got {status} {body}");

    // A numeric field: 1996, 1987 and 1972.
    let year = &body["stats"]["year"];
    assert_eq!(
        (
            year["count"].as_i64(),
            year["missing"].as_i64(),
            year["min"].as_str(),
            year["max"].as_str(),
            year["mean"].as_str(),
            year["sum"].as_f64(),
            year["sumOfSquares"].as_f64(),
        ),
        (
            Some(3),
            Some(0),
            Some("1972"),
            Some("1996"),
            Some("1985"),
            Some(5955.0),
            Some(11_820_969.0),
        ),
        "case \"numeric statistics\": unexpected statistics for year, got {body}"
    );
    let stddev = year["stddev"].as_f64().unwrap_or_default();
    assert!(
        (stddev - 98f64.sqrt()).abs() < 1e-9,
        "case \"numeric statistics\": expected stddev {}, got {stddev}",
        98f64.sqrt()
    );

    // A field only one document holds: the other two are `missing`.
    let runtime = &body["stats"]["runtime"];
    assert_eq!(
        (
            runtime["count"].as_i64(),
            runtime["missing"].as_i64(),
            runtime["min"].as_str(),
            runtime["max"].as_str(),
            runtime["stddev"].as_f64(),
        ),
        (Some(1), Some(2), Some("242"), Some("242"), Some(0.0)),
        "case \"statistics of a field most documents lack\": got {body}"
    );

    // A non-numeric field has no mean, and min/max are the first and last value.
    let director = &body["stats"]["director"];
    assert_eq!(
        (
            director["count"].as_i64(),
            director["min"].as_str(),
            director["max"].as_str(),
            director["mean"].as_str(),
        ),
        (Some(3), Some("Branagh"), Some("Tarkovsky"), None),
        "case \"statistics of a text field\": got {body}"
    );
}

#[tokio::test]
async fn sort_orders_the_result_set_by_the_requested_keys() {
    let router = router();
    seed(&router).await;

    let cases: &[(&str, &str, &[&str])] = &[
        ("numeric ascending", "year asc", &["doc3", "doc2", "doc1"]),
        ("numeric descending", "year desc", &["doc1", "doc2", "doc3"]),
        (
            "lexicographic ascending",
            "director asc",
            &["doc1", "doc2", "doc3"],
        ),
        (
            "lexicographic descending",
            "director desc",
            &["doc3", "doc2", "doc1"],
        ),
        ("by document id descending", "_id desc", &["doc3", "doc2", "doc1"]),
        (
            "a document with no value for the key sorts last",
            "runtime asc",
            &["doc1", "doc2", "doc3"],
        ),
        (
            "several keys, the first deciding",
            "genre asc,year desc",
            &["doc2", "doc1", "doc3"],
        ),
    ];

    for (case, sort, expected) in cases {
        let (status, body) = search(
            &router,
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("sort", sort),
                ("size", "10"),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            hit_ids(&body),
            *expected,
            "case {case:?}: expected order {expected:?}, got {body}"
        );
    }

    // The default order is `_score desc`: the document the query matches twice
    // ranks above the one it matches once.
    let (status, body) = search(&router, &[("q", "|drama |tragedy")]).await;
    assert_eq!(status, 200, "default relevance order: {body}");
    assert_eq!(
        hit_ids(&body),
        ["doc1", "doc3"],
        "case \"default _score desc order\": expected the best-matching document \
         first, got {body}"
    );
}

#[tokio::test]
async fn return_selects_the_fields_of_a_hit() {
    let router = router();
    seed(&router).await;

    let cases: &[ReturnCase<'_>] = &[
        (
            "every stored field by default",
            None,
            Some(&["director", "genre", "runtime", "title", "year"]),
        ),
        (
            "_all_fields is the default spelled out",
            Some("_all_fields"),
            Some(&["director", "genre", "runtime", "title", "year"]),
        ),
        ("_no_fields returns ids only", Some("_no_fields"), None),
        (
            "a named subset",
            Some("title,year"),
            Some(&["title", "year"]),
        ),
        (
            "a named field no document holds is simply absent",
            Some("title,nosuchfield"),
            Some(&["title"]),
        ),
        ("_score returns the relevance score", Some("_score"), Some(&["_score"])),
    ];

    for (case, spec, expected) in cases {
        let mut params: Vec<(&str, &str)> = vec![("q", "title:hamlet"), ("q.parser", "lucene")];
        if let Some(spec) = spec {
            params.push(("return", spec));
        }
        let (status, body) = search(&router, &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            hit_ids(&body),
            ["doc1"],
            "case {case:?}: expected the one matching hit, got {body}"
        );
        let hit = &body["hits"]["hit"][0];
        match expected {
            None => assert!(
                hit["fields"].is_null(),
                "case {case:?}: expected no fields, got {body}"
            ),
            Some(names) => {
                let mut actual: Vec<String> = hit["fields"]
                    .as_object()
                    .map(|fields| fields.keys().cloned().collect())
                    .unwrap_or_default();
                actual.sort();
                assert_eq!(
                    actual, *names,
                    "case {case:?}: expected fields {names:?}, got {body}"
                );
            }
        }
    }

    // The values of a returned field are the values that were uploaded.
    let (_, body) = search(
        &router,
        &[("q", "title:hamlet"), ("q.parser", "lucene"), ("return", "genre")],
    )
    .await;
    let genres: Vec<String> = body["hits"]["hit"][0]["fields"]["genre"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        genres,
        ["drama", "tragedy"],
        "case \"a returned multi-valued field\": expected both uploaded values, got {body}"
    );
}

#[tokio::test]
async fn highlight_marks_the_matched_terms_in_the_requested_field() {
    let router = router();
    seed(&router).await;

    let cases: &[(&str, &str, &str, &str)] = &[
        ("default html format", "hamlet", r#"{"title":{}}"#, "<em>Hamlet</em>"),
        (
            "text format marks a match with an asterisk",
            "hamlet",
            r#"{"title":{"format":"text"}}"#,
            "*Hamlet*",
        ),
        (
            "custom tags",
            "hamlet",
            r#"{"title":{"pre_tag":"[","post_tag":"]"}}"#,
            "[Hamlet]",
        ),
    ];

    for (case, query, highlight, expected) in cases {
        let (status, body) = search(&router, &[("q", query), ("highlight", highlight)]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["hits"]["hit"][0]["highlights"]["title"].as_str(),
            Some(*expected),
            "case {case:?}: expected highlight {expected:?}, got {body}"
        );
    }

    // A field the query did not match carries no highlight.
    let (_, body) = search(
        &router,
        &[("q", "hamlet"), ("highlight", r#"{"director":{}}"#)],
    )
    .await;
    assert!(
        body["hits"]["hit"][0]["highlights"].is_null(),
        "case \"a field with no match\": expected no highlights, got {body}"
    );
}

#[tokio::test]
async fn suggest_prefix_matches_the_suggester_field_over_the_index() {
    let router = router();
    seed(&router).await;

    // (case, suggester, q, size, expected (suggestion, id) pairs, expected found)
    let cases: &[SuggestCase<'_>] = &[
        (
            "a prefix of one document's title",
            "title",
            "ha",
            "5",
            &[("Hamlet", "doc1")],
            1,
        ),
        (
            "prefix matching is case-insensitive",
            "title",
            "HAM",
            "5",
            &[("Hamlet", "doc1")],
            1,
        ),
        (
            "a suggester named for another field of the same documents",
            "director",
            "bran",
            "5",
            &[("Branagh", "doc1")],
            1,
        ),
        (
            "a one-character prefix",
            "title",
            "h",
            "5",
            &[("Hamlet", "doc1")],
            1,
        ),
        (
            "a shared value scores by how many documents suggest it",
            "genre",
            "dr",
            "5",
            &[("drama", "doc1"), ("drama", "doc3")],
            2,
        ),
        (
            "size truncates the suggestions but not found",
            "genre",
            "dr",
            "1",
            &[("drama", "doc1")],
            2,
        ),
        (
            "a prefix no value starts with suggests nothing",
            "title",
            "zzz",
            "5",
            &[],
            0,
        ),
        (
            "the beginning of the field must match, not the middle",
            "title",
            "comedy",
            "5",
            &[],
            0,
        ),
    ];

    for (case, suggester, query, size, expected, found) in cases {
        let (status, body) = suggest(
            &router,
            &[("suggester", suggester), ("q", query), ("size", size)],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["suggest"]["found"].as_i64(),
            Some(*found),
            "case {case:?}: expected found {found}, got {body}"
        );
        assert_eq!(
            body["suggest"]["query"].as_str(),
            Some(*query),
            "case {case:?}: expected the query echoed in the suggest model, got {body}"
        );
        let actual: Vec<(String, String)> = body["suggest"]["suggestions"]
            .as_array()
            .map(|matches| {
                matches
                    .iter()
                    .map(|m| {
                        (
                            m["suggestion"].as_str().unwrap_or_default().to_string(),
                            m["id"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(suggestion, id)| ((*suggestion).to_string(), (*id).to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected suggestions {expected:?}, got {body}"
        );
    }

    // RANKING: a suggestion more documents offer ranks ahead of one fewer
    // documents offer, and carries the higher score. The score is this
    // endpoint's own popularity count (see `Suggest`'s handler), so what is
    // asserted here is the ORDER and the relation between the scores, never a
    // particular number.
    let (status, body) = upload(
        &router,
        "application/json",
        r#"[{"type":"add","id":"doc4","fields":{"genre":["documentary"]}}]"#,
    )
    .await;
    assert_eq!(
        (status, body["adds"].as_i64()),
        (200, Some(1)),
        "case \"a document offering a less common suggestion\": got {status} {body}"
    );
    let (_, body) = suggest(&router, &[("suggester", "genre"), ("q", "d")]).await;
    let ranked: Vec<(String, i64)> = body["suggest"]["suggestions"]
        .as_array()
        .map(|matches| {
            matches
                .iter()
                .map(|m| {
                    (
                        m["suggestion"].as_str().unwrap_or_default().to_string(),
                        m["score"].as_i64().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // `drama` is offered by two documents, `documentary` by one, and a
    // suggestion is reported per document that offers it.
    assert_eq!(
        ranked.iter().map(|(value, _)| value.as_str()).collect::<Vec<_>>(),
        vec!["drama", "drama", "documentary"],
        "case \"the more widely offered suggestion ranks first\": got {body}"
    );
    let (_, best) = &ranked[0];
    let (_, least) = &ranked[ranked.len() - 1];
    assert!(
        best > least && *least > 0,
        "case \"a more widely offered suggestion scores higher\": expected \
         {best:?} > {least:?} > 0, got {body}"
    );
}

#[tokio::test]
async fn a_deleted_document_stops_matching_search_and_suggest() {
    let router = router();
    seed(&router).await;

    let (status, body) = search(&router, &[("q", "comedy")]).await;
    assert_eq!(status, 200, "before the delete: {body}");
    assert_eq!(
        hit_ids(&body),
        ["doc2"],
        "case \"before the delete\": expected doc2 to match, got {body}"
    );

    let (status, body) = upload(
        &router,
        "application/json",
        r#"[{"type":"delete","id":"doc2"}]"#,
    )
    .await;
    assert_eq!(status, 200, "the delete batch: {body}");
    assert_eq!(
        (body["adds"].as_i64(), body["deletes"].as_i64()),
        (Some(0), Some(1)),
        "case \"a delete-only batch\": expected 0 adds and 1 delete, got {body}"
    );

    let (_, body) = search(&router, &[("q", "comedy")]).await;
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(0),
        "case \"after the delete\": expected no hits, got {body}"
    );
    let (_, body) = suggest(&router, &[("suggester", "title"), ("q", "The")]).await;
    assert_eq!(
        body["suggest"]["found"].as_i64(),
        Some(0),
        "case \"after the delete\": expected no suggestions, got {body}"
    );

    // Re-uploading the same id restores it, and replaces its stored fields.
    let (_, body) = upload(
        &router,
        "application/json",
        r#"[{"type":"add","id":"doc2","fields":{"title":"Comedy restored"}}]"#,
    )
    .await;
    assert_eq!(
        body["adds"].as_i64(),
        Some(1),
        "case \"re-adding a deleted document\": got {body}"
    );
    let (_, body) = search(&router, &[("q", "restored")]).await;
    assert_eq!(
        hit_ids(&body),
        ["doc2"],
        "case \"re-adding a deleted document\": expected it to match again, got {body}"
    );
    let (_, body) = search(&router, &[("q", "comedy")]).await;
    assert_eq!(
        hit_ids(&body),
        ["doc2"],
        "case \"an add replaces the stored document\": got {body}"
    );
    let fields = &body["hits"]["hit"][0]["fields"];
    assert!(
        fields["genre"].is_null(),
        "case \"an add replaces the stored document\": the replaced document must not \
         keep its old fields, got {body}"
    );

    // A suggester is the field it suggests from, derived from the documents the
    // index holds, so deleting every document that carries a field removes its
    // suggester: it does not linger for a field nothing holds any more.
    let (status, body) = suggest(&router, &[("suggester", "director"), ("q", "bran")]).await;
    assert_eq!(
        (status, body["suggest"]["found"].as_i64()),
        (200, Some(1)),
        "case \"a suggester of a field still held\": got {body}"
    );
    let (status, body) = upload(
        &router,
        "application/json",
        r#"[{"type":"delete","id":"doc1"},{"type":"delete","id":"doc3"}]"#,
    )
    .await;
    assert_eq!(
        (status, body["deletes"].as_i64()),
        (200, Some(2)),
        "deleting the documents that carry the director field: got {body}"
    );
    let (status, body) = suggest(&router, &[("suggester", "director"), ("q", "bran")]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "SearchException"),
        "case \"a suggester whose field no document carries any more\": got {body}"
    );
}

#[tokio::test]
async fn a_batch_reports_what_the_document_service_ignored() {
    let router = router();
    let (status, body) = upload(
        &router,
        "application/json",
        r#"[{"type":"add","id":"doc1","version":3,"fields":{"title":"Hamlet","note":null}},
            {"type":"delete","id":"doc2","fields":{"title":"ignored"}}]"#,
    )
    .await;
    assert_eq!(status, 200, "a batch with ignored input: {body}");
    assert_eq!(
        (body["adds"].as_i64(), body["deletes"].as_i64()),
        (Some(1), Some(1)),
        "case \"a batch with ignored input\": got {body}"
    );
    let warnings: Vec<String> = body["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .map(|w| w["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        warnings.len(),
        3,
        "case \"a batch with ignored input\": expected one warning for the unknown \
         entry member, one for the null value and one for the delete's fields, got {body}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("note")),
        "case \"a null field value\": expected a warning naming the field, got {warnings:?}"
    );
    // The null-valued field was not stored, and the rest of the document was.
    let (_, body) = search(&router, &[("q", "hamlet"), ("return", "_all_fields")]).await;
    let fields = &body["hits"]["hit"][0]["fields"];
    assert!(
        fields["note"].is_null() && fields["title"].is_array(),
        "case \"a null field value\": expected only the title stored, got {body}"
    );

    // A delete entry's `fields` object is accepted, ignored and reported as a
    // warning rather than refused, whichever shape the entry carries.
    let second_router = self::router();
    let (status, body) = upload(
        &second_router,
        "application/json",
        r#"[{"type":"add","id":"x1","fields":{"title":"Hamlet"}},
            {"type":"delete","id":"x2","fields":{"title":"ignored"}}]"#,
    )
    .await;
    assert_eq!(
        (status, body["adds"].as_i64(), body["deletes"].as_i64()),
        (200, Some(1), Some(1)),
        "case \"a delete carrying a field\": got {status} {body}"
    );
    let warnings: Vec<String> = body["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .map(|w| w["message"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        warnings.iter().any(|w| w.contains("x2")),
        "case \"a delete carrying a field\": expected a warning naming the \
         delete entry, got {warnings:?}"
    );
}

#[tokio::test]
async fn a_refused_request_answers_the_modelled_error_with_http_400() {
    let router = router();
    seed(&router).await;

    // (case, body of a batch) — every row breaks a documented rule of the format.
    let long_id = "x".repeat(129);
    let batches: &[(&str, &str, &str)] = &[
        ("a content type the model does not declare", "application/yaml", "[]"),
        ("a body that is not JSON", "application/json", "{not json"),
        ("a batch that is not an array", "application/json", "{}"),
        (
            "an entry that is not an object",
            "application/json",
            r#"["add"]"#,
        ),
        (
            "an entry with no type",
            "application/json",
            r#"[{"id":"doc9","fields":{"title":"x"}}]"#,
        ),
        (
            "an entry with an unknown type",
            "application/json",
            r#"[{"type":"patch","id":"doc9","fields":{"title":"x"}}]"#,
        ),
        (
            "an entry with no id",
            "application/json",
            r#"[{"type":"add","fields":{"title":"x"}}]"#,
        ),
        (
            "an id with a character ids may not contain",
            "application/json",
            r#"[{"type":"add","id":"doc 9","fields":{"title":"x"}}]"#,
        ),
        (
            "an add with no fields",
            "application/json",
            r#"[{"type":"add","id":"doc9","fields":{}}]"#,
        ),
        (
            "a field name that does not start with a lower-case letter",
            "application/json",
            r#"[{"type":"add","id":"doc9","fields":{"Title":"x"}}]"#,
        ),
        (
            "the reserved field name",
            "application/json",
            r#"[{"type":"add","id":"doc9","fields":{"score":"1"}}]"#,
        ),
        (
            "a field value that is neither a string nor a number",
            "application/json",
            r#"[{"type":"add","id":"doc9","fields":{"title":{"a":1}}}]"#,
        ),
        (
            "the content type this endpoint does not serve",
            "application/xml",
            "<batch><add id=\"doc9\"><field name=\"title\">x</field></add></batch>",
        ),
    ];
    for (case, content_type, batch) in batches {
        let (status, body) = upload(&router, content_type, batch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DocumentServiceException",
            "case {case:?}: expected the modelled error, got {body}"
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }
    // A refused batch leaves the index untouched.
    let (_, body) = search(&router, &[("q", "matchall"), ("q.parser", "structured")]).await;
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(3),
        "case \"a refused batch\": the index must be unchanged, got {body}"
    );
    // An id of 129 characters is refused; one of 128 is accepted.
    let (status, body) = upload(
        &router,
        "application/json",
        &format!(r#"[{{"type":"add","id":"{long_id}","fields":{{"title":"x"}}}}]"#),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DocumentServiceException"),
        "case \"an id one character past the documented maximum\": got {body}"
    );
    let (status, body) = upload(
        &router,
        "application/json",
        &format!(
            r#"[{{"type":"add","id":"{}","fields":{{"title":"x"}}}}]"#,
            "x".repeat(128)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an id of exactly the documented maximum\": got {body}"
    );

    // Search requests this endpoint refuses.
    let searches: &[(&str, &[(&str, &str)])] = &[
        ("a query parser the model does not declare", &[("q", "x"), ("q.parser", "bogus")]),
        ("an empty query", &[("q", "")]),
        (
            "a structured query with an unbalanced parenthesis",
            &[("q", "(and title:'x'"), ("q.parser", "structured")],
        ),
        (
            "a structured operator this endpoint does not evaluate",
            &[("q", "(near field=title 'x y')"), ("q.parser", "structured")],
        ),
        (
            "a range expression this endpoint does not evaluate",
            &[("q", "year:{1990,2000]"), ("q.parser", "structured")],
        ),
        (
            "a malformed filter query",
            &[("q", "matchall"), ("q.parser", "structured"), ("fq", "(or ")],
        ),
        (
            "an unterminated quote in a structured query",
            &[("q", "(term field=title 'hamlet)"), ("q.parser", "structured")],
        ),
        (
            "an unterminated quote in a simple query",
            &[("q", "\"hamlet drama"), ("q.parser", "simple")],
        ),
        (
            "an unterminated quote in a lucene query",
            &[("q", "title:\"hamlet drama"), ("q.parser", "lucene")],
        ),
        ("a sort key with no direction", &[("q", "hamlet"), ("sort", "year")]),
        (
            "a sort direction that is neither asc nor desc",
            &[("q", "hamlet"), ("sort", "year up")],
        ),
        (
            "a facet option that does not exist",
            &[("q", "hamlet"), ("facet", r#"{"genre":{"bogus":1}}"#)],
        ),
        (
            "a facet range bucket this endpoint does not evaluate",
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("facet", r#"{"year":{"buckets":["[1970,1979]"]}}"#),
            ],
        ),
        (
            "a facet open range bucket this endpoint does not evaluate",
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("facet", r#"{"year":{"buckets":["{1980,1989}"]}}"#),
            ],
        ),
        (
            "a facet parameter that is not JSON",
            &[("q", "hamlet"), ("facet", "genre")],
        ),
        (
            "a statistics option, of which there are none",
            &[("q", "hamlet"), ("stats", r#"{"year":{"sort":"count"}}"#)],
        ),
        (
            "a highlight option that does not exist",
            &[("q", "hamlet"), ("highlight", r#"{"title":{"bogus":1}}"#)],
        ),
        (
            "q.options that is not JSON",
            &[("q", "hamlet"), ("q.options", "fields")],
        ),
        (
            "a default operator that does not exist",
            &[("q", "hamlet"), ("q.options", r#"{"defaultOperator":"maybe"}"#)],
        ),
        (
            "the mutually exclusive cursor and start parameters",
            &[("q", "hamlet"), ("cursor", "initial"), ("start", "1")],
        ),
        ("a cursor this endpoint did not mint", &[("q", "hamlet"), ("cursor", "nope")]),
        (
            // A minted token carries its offset, so one naming an offset past the
            // largest index a domain can hold is refused rather than walked.
            "a cursor resuming past the documents a domain holds",
            &[
                ("q", "hamlet"),
                ("cursor", &format!("cs{:x}", MAX_DOCUMENTS_PER_DOMAIN + 1)),
            ],
        ),
        ("a negative size", &[("q", "hamlet"), ("size", "-1")]),
        ("a negative start", &[("q", "hamlet"), ("start", "-1")]),
        (
            "an expression this endpoint does not evaluate",
            &[("q", "hamlet"), ("expr", r#"{"boost":"_score*2"}"#)],
        ),
    ];
    for (case, params) in searches {
        let (status, body) = search(&router, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "SearchException",
            "case {case:?}: expected the modelled error, got {body}"
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }

    // And a suggester the domain does not have.
    let (status, body) = suggest(&router, &[("suggester", "nosuchsuggester"), ("q", "ha")]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "SearchException"),
        "case \"a suggester the domain does not have\": got {body}"
    );
    // Including a name the index could have invented but did not: a suggester is
    // the field it suggests from, not a name derived from one.
    let (status, body) = suggest(&router, &[("suggester", "title_suggester"), ("q", "ha")]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "SearchException"),
        "case \"a suggester name derived from a field name\": got {body}"
    );
    // A query that names no prefix is refused on BOTH surfaces, with the same
    // modelled error: every stored value begins with the empty prefix, so serving
    // it would answer "every document" to a request that asked for nothing.
    let empty_queries: &[(&str, &str)] = &[
        ("an empty query", ""),
        ("a whitespace-only query", "   "),
    ];
    for (case, query) in empty_queries {
        let (status, body) = suggest(&router, &[("suggester", "title"), ("q", query)]).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "SearchException"),
            "case {case:?} on Suggest: got {status} {body}"
        );
        let (status, body) = search(&router, &[("q", query)]).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "SearchException"),
            "case {case:?} on Search: got {status} {body}"
        );
    }
}

/// The batch parser reads caller-controlled bytes, so the two documented size
/// ceilings are refusals rather than reads: the whole batch ("The entire batch
/// cannot exceed 5 MB") and one document ("Individual documents cannot contain
/// more than 1 MB of data"). A batch or document AT a ceiling is still applied,
/// and a refused one applies nothing.
#[tokio::test]
async fn an_oversized_batch_or_document_is_refused_and_applies_nothing() {
    let router = router();
    seed(&router).await;

    // One `add` entry whose stored data is `payload` bytes of field value plus
    // the field name, which is what the per-document ceiling measures.
    let document = |payload: usize| {
        format!(
            r#"[{{"type":"add","id":"big","fields":{{"title":"{}"}}}}]"#,
            "x".repeat(payload)
        )
    };
    let field_name = "title".len();
    let at_document_bound = document(MAX_DOCUMENT_BYTES - field_name);
    let past_document_bound = document(MAX_DOCUMENT_BYTES - field_name + 1);
    // A batch of exactly `total` bytes holding ONE small document, padded with
    // the whitespace JSON allows between values, so the whole-batch ceiling is
    // tested on its own rather than through a huge document.
    let entry = r#"[{"type":"add","id":"pad","fields":{"title":"padded"}}]"#;
    let batch_of = |total: usize| {
        let mut body = String::with_capacity(total);
        body.push_str(&entry[..entry.len() - 1]);
        body.push_str(&" ".repeat(total - entry.len()));
        body.push(']');
        body
    };
    let at_batch_bound = batch_of(MAX_BATCH_BYTES);
    let past_batch_bound = batch_of(MAX_BATCH_BYTES + 1);

    let refused: &[(&str, &str)] = &[
        ("a document one byte past the 1 MB ceiling", &past_document_bound),
        ("a batch one byte past the 5 MB ceiling", &past_batch_bound),
    ];
    for (case, batch) in refused {
        let (status, body) = upload(&router, "application/json", batch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DocumentServiceException",
            "case {case:?}: expected the modelled error, got {body}"
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }
    // Neither refusal applied anything: the index still holds only the seed.
    let (_, body) = search(
        &router,
        &[("q", "matchall"), ("q.parser", "structured"), ("size", "50")],
    )
    .await;
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(3),
        "case \"an oversized batch\": the index must be unchanged, got {body}"
    );

    let accepted: &[(&str, &str)] = &[
        ("a document of exactly the 1 MB ceiling", &at_document_bound),
        ("a batch of exactly the 5 MB ceiling", &at_batch_bound),
    ];
    for (case, batch) in accepted {
        let (status, body) = upload(&router, "application/json", batch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["adds"].as_i64(),
            Some(1),
            "case {case:?}: expected one add applied, got {body}"
        );
    }
    let (_, body) = search(
        &router,
        &[("q", "matchall"), ("q.parser", "structured"), ("size", "50")],
    )
    .await;
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(5),
        "case \"a batch at the ceilings\": both documents must be indexed, got {body}"
    );
}

/// The two Lucene-family parsers DEFINE modifiers this endpoint does not
/// evaluate — a boost, a fuzzy or proximity distance, an interior wildcard and a
/// regular expression — so a query carrying one is refused with the modelled
/// `SearchException` rather than read as ordinary text and answered with a result
/// set the request did not ask for. The prefix search they do define still works,
/// the simple parser gives the same characters no meaning (its text analysis
/// drops them), and an apostrophe is part of the word it sits in rather than a
/// quote that swallows the rest of the query.
#[tokio::test]
async fn lucene_modifiers_this_endpoint_does_not_evaluate_are_refused() {
    let router = router();
    seed(&router).await;
    // A title carrying an apostrophe, so the term splitter's handling of one is
    // observable: its tokens are "don", "t", "look", "up".
    let (status, body) = upload(
        &router,
        "application/json",
        r#"[{"type":"add","id":"doc4","fields":{"title":"Don't Look Up","genre":["comedy"]}},
            {"type":"add","id":"doc5","fields":{"title":"Macbeth (Live)","genre":["drama"]}}]"#,
    )
    .await;
    assert_eq!(status, 200, "indexing the apostrophe document: {body}");

    // (case, parameters) — each names a modifier the parser defines and this
    // endpoint does not evaluate, under both Lucene-family parsers.
    let refused: &[(&str, &[(&str, &str)])] = &[
        (
            "a lucene boost",
            &[("q", "title:hamlet^2"), ("q.parser", "lucene")],
        ),
        (
            "a dismax boost",
            &[("q", "hamlet^2"), ("q.parser", "dismax")],
        ),
        (
            "a lucene fuzzy distance",
            &[("q", "hamlet~1"), ("q.parser", "lucene")],
        ),
        (
            "a dismax phrase proximity",
            &[("q", "\"hamlet drama\"~5"), ("q.parser", "dismax")],
        ),
        (
            "a lucene single-character wildcard",
            &[("q", "h?mlet"), ("q.parser", "lucene")],
        ),
        (
            "a lucene wildcard inside a term",
            &[("q", "h*let"), ("q.parser", "lucene")],
        ),
        (
            "a lucene regular expression",
            &[("q", "/h.*t/"), ("q.parser", "lucene")],
        ),
        // Grouping re-associates the booleans around it, which this endpoint's
        // flat term parser does not do, so a grouped query must be refused
        // rather than answered with `(genre` read as a field name.
        (
            "a lucene grouped boolean",
            &[
                ("q", "(genre:drama OR genre:comedy) AND title:hamlet"),
                ("q.parser", "lucene"),
            ],
        ),
        (
            "a dismax grouped boolean",
            &[("q", "(drama | comedy) hamlet"), ("q.parser", "dismax")],
        ),
        (
            "a lucene group around a single term",
            &[("q", "(hamlet)"), ("q.parser", "lucene")],
        ),
        (
            "a lucene group left unclosed",
            &[("q", "(hamlet OR drama"), ("q.parser", "lucene")],
        ),
        (
            "a lucene group inside a fielded term",
            &[("q", "title:(hamlet OR macbeth)"), ("q.parser", "lucene")],
        ),
    ];
    for (case, params) in refused {
        let (status, body) = search(&router, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "SearchException",
            "case {case:?}: expected the modelled error, got {body}"
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }

    // (case, parameters, the hit ids expected in order) — what the same
    // characters mean where the parser gives them no special meaning, and the
    // prefix search the Lucene parser does evaluate.
    let answered: &[QueryCase<'_>] = &[
        (
            "a lucene trailing prefix is still evaluated",
            &[("q", "title:ham*"), ("q.parser", "lucene")],
            &["doc1"],
        ),
        (
            "a caret is ordinary text to the simple parser",
            &[("q", "hamlet^2")],
            &[],
        ),
        (
            "a tilde is ordinary text to the simple parser",
            &[("q", "hamlet~1")],
            &[],
        ),
        (
            "an apostrophe is part of its word, not a quote",
            &[("q", "don't look")],
            &["doc4"],
        ),
        (
            "an apostrophe does not swallow the terms after it",
            &[("q", "don't comedy")],
            &["doc4"],
        ),
        // A parenthesis INSIDE a quoted phrase is literal text of the phrase,
        // which this endpoint evaluates, so it is answered rather than refused.
        (
            "a lucene quoted phrase may carry parentheses",
            &[("q", "\"macbeth (live)\""), ("q.parser", "lucene")],
            &["doc5"],
        ),
        (
            "a lucene fielded quoted phrase may carry parentheses",
            &[
                ("q", "title:\"macbeth (live)\""),
                ("q.parser", "lucene"),
            ],
            &["doc5"],
        ),
        (
            "parentheses are ordinary text to the simple parser",
            &[("q", "(macbeth)")],
            &["doc5"],
        ),
    ];
    for (case, params, expected) in answered {
        let (status, body) = search(&router, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            hit_ids(&body),
            *expected,
            "case {case:?}: expected hits {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn every_answer_carries_its_own_status() {
    let router = router();
    seed(&router).await;

    // `timems` must be a MEASUREMENT, so each call is timed here too: the value
    // the response reports cannot exceed the wall-clock time the call took (plus
    // the millisecond the service rounds up to). It can be 0, because an answer
    // computed out of this process's memory really can take less than a
    // millisecond.
    let started = std::time::Instant::now();
    let (_, first) = search(&router, &[("q", "matchall"), ("q.parser", "structured")]).await;
    let first_wall = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX) + 1;
    let started = std::time::Instant::now();
    let (_, second) = search(&router, &[("q", "hamlet")]).await;
    let second_wall = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX) + 1;
    let started = std::time::Instant::now();
    let (_, suggested) = suggest(&router, &[("suggester", "title"), ("q", "ha")]).await;
    let suggest_wall = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX) + 1;

    for (case, body, status, wall) in [
        ("search", &first, &first["status"], first_wall),
        ("a second search", &second, &second["status"], second_wall),
        ("suggest", &suggested, &suggested["status"], suggest_wall),
    ] {
        assert!(
            status["timems"].is_i64() || status["timems"].is_u64(),
            "case {case:?}: expected a timems member, got {body}"
        );
        let timems = status["timems"].as_i64().unwrap_or(-1);
        assert!(
            (0..=wall).contains(&timems),
            "case {case:?}: expected a measured timems within the {wall} ms the \
             call took, got {timems} in {body}"
        );
        assert!(
            !status["rid"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: expected a generated rid, got {body}"
        );
    }
    assert_ne!(
        first["status"]["rid"].as_str(),
        second["status"]["rid"].as_str(),
        "case \"two searches\": each response must carry its own rid"
    );
}

#[tokio::test]
async fn two_domains_do_not_share_an_index() {
    let router = router();
    seed(&router).await;

    // The `doc-` endpoint wrote the index the `search-` endpoint of the SAME
    // domain reads.
    let (status, body) = search(&router, &[("q", "matchall"), ("q.parser", "structured")]).await;
    assert_eq!(status, 200, "the domain that was uploaded to: {body}");
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(3),
        "case \"the domain that was uploaded to\": got {body}"
    );

    // Another domain's endpoint sees none of it.
    let (status, body) = get(
        &router,
        OTHER_HOST,
        "/2013-01-01/search",
        &[("q", "matchall"), ("q.parser", "structured")],
    )
    .await;
    assert_eq!(status, 200, "another domain: {body}");
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(0),
        "case \"another domain's endpoint\": expected an empty index, got {body}"
    );

    // And a batch uploaded to that other domain stays there.
    let (_, body) = upload_to(
        &router,
        "doc-books-9z8y7x.us-east-1.cloudsearchdomain.amazonaws.com",
        "application/json",
        r#"[{"type":"add","id":"b1","fields":{"title":"Hamlet"}}]"#,
    )
    .await;
    assert_eq!(
        body["adds"].as_i64(),
        Some(1),
        "uploading to the other domain: got {body}"
    );
    let (_, body) = get(
        &router,
        OTHER_HOST,
        "/2013-01-01/search",
        &[("q", "hamlet")],
    )
    .await;
    assert_eq!(
        hit_ids(&body),
        ["b1"],
        "case \"the other domain's own document\": got {body}"
    );
    let (_, body) = search(&router, &[("q", "hamlet")]).await;
    assert_eq!(
        hit_ids(&body),
        ["doc1"],
        "case \"the first domain after the other was written\": got {body}"
    );
}

/// How a sort field compares is decided ONCE for the whole result set — numeric
/// only when every value of it is a finite number — so the comparison is the
/// same for every pair of hits. Choosing per pair would not be transitive
/// (`10 < 9` numerically while `10 < 1a < 9` lexicographically) and would
/// misorder a page.
#[tokio::test]
async fn a_sort_field_orders_consistently_whatever_its_values_look_like() {
    // (case, the value stored under `code` per document — `None` for a document
    // that has no `code` field at all — and the document ids in ascending order)
    let cases: &[SortCase<'_>] = &[
        (
            "every value is a number, so the field orders numerically",
            &[Some("10"), Some("9"), Some("2")],
            &["d2", "d1", "d0"],
        ),
        (
            "one value is not a number, so the field orders lexicographically",
            &[Some("10"), Some("9"), Some("1a")],
            &["d0", "d2", "d1"],
        ),
        (
            "non-finite spellings are not numbers either",
            &[Some("10"), Some("NaN"), Some("inf")],
            &["d0", "d1", "d2"],
        ),
        (
            "a document missing the field sorts after one that has it",
            &[Some("10"), Some("2"), None],
            &["d1", "d0", "d2"],
        ),
    ];
    for (case, values, expected) in cases {
        let router = router();
        let mut batch = String::from("[");
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                batch.push(',');
            }
            let code = match value {
                Some(value) => format!(r#","code":"{value}""#),
                None => String::new(),
            };
            batch.push_str(&format!(
                r#"{{"type":"add","id":"d{index}","fields":{{"title":"movie"{code}}}}}"#
            ));
        }
        batch.push(']');
        let (status, body) = upload(&router, "application/json", &batch).await;
        assert_eq!(status, 200, "case {case:?}: seeding: got {status} {body}");

        let (status, body) = search(
            &router,
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("sort", "code asc"),
                ("size", "50"),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            hit_ids(&body),
            *expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A field value is stored exactly as the batch wrote it, whatever its shape: a
/// single string, a number, a list of strings (two values of one field), and a
/// value carrying characters the JSON form escapes.
#[tokio::test]
async fn a_field_value_is_stored_as_the_batch_wrote_it() {
    let router = router();
    let batch = r#"[
      {"type":"add","id":"e1","fields":{"title":"Hamlet & Ophelia"}},
      {"type":"add","id":"e2","fields":{"title":"A \"quoted\" title"}},
      {"type":"add","id":"e3","fields":{"title":"line\nbreak"}},
      {"type":"add","id":"e4","fields":{"genre":["drama","tragedy"]}},
      {"type":"add","id":"e5","fields":{"title":"  Indented  "}},
      {"type":"add","id":"e6","fields":{"year":1996,"rating":7.5}}
    ]"#;
    let (status, body) = upload(&router, "application/json", batch).await;
    assert_eq!(
        (status, body["adds"].as_i64()),
        (200, Some(6)),
        "uploading the batch: got {status} {body}"
    );

    // (case, document id, field, the values the index must hold)
    let cases: &[(&str, &str, &str, &[&str])] = &[
        (
            "a character the JSON form escapes",
            "e1",
            "title",
            &["Hamlet & Ophelia"],
        ),
        ("an escaped quote inside a value", "e2", "title", &["A \"quoted\" title"]),
        ("an escaped newline inside a value", "e3", "title", &["line\nbreak"]),
        (
            "two values of one field",
            "e4",
            "genre",
            &["drama", "tragedy"],
        ),
        (
            "whitespace around a value, which is part of it",
            "e5",
            "title",
            &["  Indented  "],
        ),
        ("an integer value", "e6", "year", &["1996"]),
        ("a fractional value", "e6", "rating", &["7.5"]),
    ];
    let (_, found) = search(
        &router,
        &[
            ("q", "matchall"),
            ("q.parser", "structured"),
            ("size", "50"),
            ("return", "_all_fields"),
        ],
    )
    .await;
    let hits = found["hits"]["hit"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (case, id, field, values) in cases {
        let hit = hits
            .iter()
            .find(|hit| hit["id"].as_str() == Some(*id))
            .unwrap_or_else(|| panic!("case {case:?}: no hit for {id} in {found}"));
        let actual: Vec<String> = hit["fields"][field]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            actual, *values,
            "case {case:?}: expected {values:?}, got {actual:?} in {hit}"
        );
    }
}

/// A `Search` or `Suggest` request whose URI names a search endpoint but whose
/// `Host` header carries `host` verbatim — the header the index key is derived
/// from, and the one a caller can make arbitrary.
async fn get_with_host_header(
    router: &MultiServiceRouter,
    host: &str,
    path: &str,
    params: &[(&str, &str)],
) -> (u16, serde_json::Value) {
    let mut query = String::from("format=sdk&pretty=true");
    for (name, value) in params {
        query.push('&');
        query.push_str(name);
        query.push('=');
        query.push_str(&encode(value));
    }
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{SEARCH_HOST}{path}?{query}"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("build the request");
    send(router, request).await
}

/// `UploadDocuments` whose `Host` header carries `host` verbatim.
async fn upload_with_host_header(
    router: &MultiServiceRouter,
    host: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{DOC_HOST}/2013-01-01/documents/batch?format=sdk"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("build the UploadDocuments request")
        ;
    send(router, request).await
}

/// The index key comes from the request's `Host` header, which a caller chooses
/// freely, so every header value a client can send must answer a key — a
/// malformed host is served its own (empty) index, never a panic.
#[tokio::test]
async fn a_malformed_host_header_is_served_its_own_index_not_a_panic() {
    let router = router();

    // (case, Host header, the term only this host's document carries). The
    // bracketed rows are the authority form of an IPv6 literal and the shapes a
    // naive `]`-search mis-reads; the last rows are the well-formed endpoints.
    let hosts: &[(&str, &str, &str)] = &[
        ("a host that is a bare ']'", "]", "alpha"),
        ("a host beginning with ']'", "]x", "bravo"),
        ("a host with an interior ']'", "a]b", "charlie"),
        ("an IPv6 literal with a port", "[::1]:9297", "delta"),
        ("an unbracketed IPv6-looking host", "::1", "echo"),
        ("an empty host header", "", "foxtrot"),
        ("a host that is only a port", ":9297", "golf"),
        ("localhost with a port", "localhost:9297", "hotel"),
        (
            "a well-formed document endpoint",
            "doc-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com",
            "india",
        ),
        (
            "a well-formed search endpoint",
            "search-movies-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com",
            "juliett",
        ),
    ];
    for (index, (case, host, term)) in hosts.iter().enumerate() {
        let batch = format!(
            r#"[{{"type":"add","id":"h{index}","fields":{{"title":"{term}"}}}}]"#
        );
        let (status, body) = upload_with_host_header(&router, host, &batch).await;
        assert_eq!(
            (status, body["adds"].as_i64()),
            (200, Some(1)),
            "case {case:?}: uploading under this host must be applied, got {status} {body}"
        );

        // The same host reads back exactly what it wrote: the key is well defined
        // for every one of these values.
        let (status, body) =
            get_with_host_header(&router, host, "/2013-01-01/search", &[("q", term)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: searching under this host must answer, got {status} {body}"
        );
        assert_eq!(
            hit_ids(&body),
            [format!("h{index}")],
            "case {case:?}: expected this host's own document, got {body}"
        );

        // And Suggest, which derives the same key.
        let (status, body) = get_with_host_header(
            &router,
            host,
            "/2013-01-01/suggest",
            &[("suggester", "title"), ("q", &term[..2])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: suggesting under this host must answer, got {status} {body}"
        );
        assert!(
            body["suggest"]["found"].as_i64().unwrap_or_default() >= 1,
            "case {case:?}: expected a suggestion from this host's index, got {body}"
        );
    }
}

/// `levels` nested `(and …)` sub-expressions around one leaf term.
fn nested_query(levels: usize) -> String {
    format!(
        "{}title:'hamlet'{}",
        "(and ".repeat(levels),
        ")".repeat(levels)
    )
}

/// A query string is caller-controlled and drives tokenizing, parsing and
/// (structured) nesting, so the endpoint bounds its length and its nesting depth
/// and refuses what is past them with the modelled `SearchException` — rather
/// than recursing as deep as the caller asks.
#[tokio::test]
async fn a_query_past_its_published_bounds_is_refused_not_fatal() {
    let router = router();
    seed(&router).await;

    let at_length_bound = "a".repeat(MAX_QUERY_BYTES);
    let past_length_bound = "a".repeat(MAX_QUERY_BYTES + 1);
    // One level past the bound, and far past it: the leaf of a query nesting
    // `levels` deep sits at depth `levels + 1`.
    let just_inside_depth_bound = nested_query(MAX_QUERY_DEPTH - 1);
    let past_depth_bound = nested_query(MAX_QUERY_DEPTH + 1);
    let absurdly_deep = nested_query(MAX_QUERY_DEPTH * 20);
    // `q.options` is caller-supplied too, and every field-less term of the query
    // searches each field it names.
    let fields = |count: usize| {
        let names: Vec<String> = (0..count).map(|index| format!("\"f{index}\"")).collect();
        format!(r#"{{"fields":[{}]}}"#, names.join(","))
    };
    let at_field_bound = fields(MAX_QUERY_FIELDS);
    let past_field_bound = fields(MAX_QUERY_FIELDS + 1);
    let options_past_length_bound = format!(r#"{{"fields":["{}"]}}"#, "a".repeat(MAX_QUERY_BYTES));
    // A query with as many terms as the endpoint accepts AND the largest field
    // list: one leaf per term, each SHARING the one field list and reading each
    // document's tokens once, so this is answered rather than amplified.
    let terms = |count: usize| {
        (0..count)
            .map(|index| format!("t{index}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let at_term_bound = terms(MAX_QUERY_TERMS);
    let past_term_bound = terms(MAX_QUERY_TERMS + 1);
    // A quoted phrase is ONE leaf, but it compares as many tokens per window as it
    // holds, so its width is bounded too.
    let at_phrase_bound = format!("\"{}\"", terms(MAX_QUERY_TERMS));
    let past_phrase_bound = format!("\"{}\"", terms(MAX_QUERY_TERMS + 1));
    // `facet`, `stats` and `highlight` each cost another pass over the matching
    // documents per field they name.
    let field_specs = |count: usize| {
        let entries: Vec<String> = (0..count)
            .map(|index| format!("\"f{index}\":{{}}"))
            .collect();
        format!("{{{}}}", entries.join(","))
    };
    let at_spec_bound = field_specs(MAX_QUERY_FIELDS);
    let past_spec_bound = field_specs(MAX_QUERY_FIELDS + 1);
    let at_bucket_bound = format!(r#"{{"genre":{{"size":{MAX_FACET_SIZE}}}}}"#);
    let past_bucket_bound = format!(r#"{{"genre":{{"size":{}}}}}"#, MAX_FACET_SIZE + 1);
    let past_phrase_count_bound = format!(
        r#"{{"title":{{"max_phrases":{}}}}}"#,
        MAX_HIGHLIGHT_PHRASES + 1
    );
    // The markup a highlight emits is written by the CALLER, twice per marked
    // occurrence per hit, so a tag's length is bounded like every other
    // request-chosen factor.
    let at_tag_bound = format!(
        r#"{{"title":{{"pre_tag":"{}","post_tag":"]"}}}}"#,
        "[".repeat(MAX_HIGHLIGHT_TAG_BYTES)
    );
    let past_tag_bound = format!(
        r#"{{"title":{{"pre_tag":"{}","post_tag":"]"}}}}"#,
        "[".repeat(MAX_HIGHLIGHT_TAG_BYTES + 1)
    );
    let past_post_tag_bound = format!(
        r#"{{"title":{{"post_tag":"{}"}}}}"#,
        "]".repeat(MAX_HIGHLIGHT_TAG_BYTES + 1)
    );
    // The page a request asks for is bounded by the documented paging depth, in
    // its width and in the hit it ends at.
    let at_page_bound = MAX_PAGE_DEPTH.to_string();
    let past_page_bound = (MAX_PAGE_DEPTH + 1).to_string();
    // `sort` names one key per field, and each key reads that field of every hit.
    let sort_keys = |count: usize| {
        (0..count)
            .map(|index| format!("f{index} asc"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let at_sort_bound = sort_keys(MAX_QUERY_FIELDS);
    let past_sort_bound = sort_keys(MAX_QUERY_FIELDS + 1);
    // `return` names one field per hit of the page to project.
    let return_fields = |count: usize| {
        (0..count)
            .map(|index| format!("f{index}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let at_return_bound = return_fields(MAX_QUERY_FIELDS);
    let past_return_bound = return_fields(MAX_QUERY_FIELDS + 1);

    // Refused: 400 and the modelled error, for every parser that reads the string.
    let refused: &[(&str, &[(&str, &str)])] = &[
        (
            "a q one byte past the length bound, simple parser",
            &[("q", &past_length_bound)],
        ),
        (
            "a q past the length bound, lucene parser",
            &[("q", &past_length_bound), ("q.parser", "lucene")],
        ),
        (
            "a q past the length bound, dismax parser",
            &[("q", &past_length_bound), ("q.parser", "dismax")],
        ),
        (
            "a q past the length bound, structured parser",
            &[("q", &past_length_bound), ("q.parser", "structured")],
        ),
        (
            "an fq past the length bound",
            &[("q", "hamlet"), ("fq", &past_length_bound)],
        ),
        (
            "a structured q one level past the depth bound",
            &[("q", &past_depth_bound), ("q.parser", "structured")],
        ),
        (
            "an fq one level past the depth bound",
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("fq", &past_depth_bound),
            ],
        ),
        (
            "a structured q nesting far past the depth bound",
            &[("q", &absurdly_deep), ("q.parser", "structured")],
        ),
        (
            "q.options naming one field past the field bound",
            &[("q", "hamlet"), ("q.options", &past_field_bound)],
        ),
        (
            "q.options past the length bound",
            &[("q", "hamlet"), ("q.options", &options_past_length_bound)],
        ),
        (
            "a q.options option that changes which documents match",
            &[("q", "hamlet"), ("q.options", r#"{"operators":"[]"}"#)],
        ),
        (
            "a q.options phrase slop that changes which documents match",
            &[("q", "hamlet"), ("q.options", r#"{"phraseSlop":2}"#)],
        ),
        (
            "a q.options explicit phrase slop that changes which documents match",
            &[
                ("q", "hamlet"),
                ("q.options", r#"{"explicitPhraseSlop":1}"#),
            ],
        ),
        (
            "a q looking for one term past the term bound",
            &[("q", &past_term_bound)],
        ),
        (
            "an fq looking for one term past the term bound",
            &[
                ("q", "matchall"),
                ("q.parser", "structured"),
                ("fq", &format!("(or {past_term_bound})")),
            ],
        ),
        (
            "a q phrase one term past the term bound",
            &[("q", &past_phrase_bound)],
        ),
        (
            "a facet naming one field past the field bound",
            &[("q", "hamlet"), ("facet", &past_spec_bound)],
        ),
        (
            "a stats parameter naming one field past the field bound",
            &[("q", "hamlet"), ("stats", &past_spec_bound)],
        ),
        (
            "a highlight naming one field past the field bound",
            &[("q", "hamlet"), ("highlight", &past_spec_bound)],
        ),
        (
            "a facet size one bucket past the bucket bound",
            &[("q", "hamlet"), ("facet", &past_bucket_bound)],
        ),
        (
            "a highlight max_phrases past the bound",
            &[("q", "hamlet"), ("highlight", &past_phrase_count_bound)],
        ),
        (
            "a highlight pre_tag one byte past the tag bound",
            &[("q", "hamlet"), ("highlight", &past_tag_bound)],
        ),
        (
            "a highlight post_tag one byte past the tag bound",
            &[("q", "hamlet"), ("highlight", &past_post_tag_bound)],
        ),
        (
            "a size one hit past the paging bound",
            &[("q", "hamlet"), ("size", &past_page_bound)],
        ),
        (
            "a start and size reaching one hit past the paging bound",
            &[("q", "hamlet"), ("start", "1"), ("size", &at_page_bound)],
        ),
        (
            "a sort naming one key past the field bound",
            &[("q", "hamlet"), ("sort", &past_sort_bound)],
        ),
        (
            "a return naming one field past the field bound",
            &[("q", "hamlet"), ("return", &past_return_bound)],
        ),
    ];
    for (case, params) in refused {
        let (status, body) = search(&router, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "SearchException",
            "case {case:?}: expected the modelled error, got {body}"
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {body}"
        );
    }

    // Accepted: everything AT or inside the bounds is still answered.
    let accepted: &[BoundsCase<'_>] = &[
        (
            "a q of exactly the length bound",
            &[("q", &at_length_bound)],
            0,
        ),
        (
            "an fq of exactly the length bound",
            &[("q", "hamlet"), ("fq", &at_length_bound)],
            0,
        ),
        (
            "a structured q nesting just inside the depth bound",
            &[
                ("q", &just_inside_depth_bound),
                ("q.parser", "structured"),
            ],
            1,
        ),
        (
            "q.options naming exactly the field bound",
            &[("q", "hamlet"), ("q.options", &at_field_bound)],
            0,
        ),
        (
            "a q.options phrase slop of zero, which is what this endpoint matches",
            &[("q", "hamlet"), ("q.options", r#"{"phraseSlop":0}"#)],
            1,
        ),
        (
            "a q.options option that only boosts ranking",
            &[("q", "hamlet"), ("q.options", r#"{"tieBreaker":0.1}"#)],
            1,
        ),
        (
            "as many terms as the bound allows, searching the largest field list",
            &[("q", &at_term_bound), ("q.options", &at_field_bound)],
            0,
        ),
        (
            "a q phrase of exactly the term bound",
            &[("q", &at_phrase_bound)],
            0,
        ),
        (
            "a facet naming exactly the field bound",
            &[("q", "hamlet"), ("facet", &at_spec_bound)],
            1,
        ),
        (
            "a stats parameter naming exactly the field bound",
            &[("q", "hamlet"), ("stats", &at_spec_bound)],
            1,
        ),
        (
            "a highlight naming exactly the field bound",
            &[("q", "hamlet"), ("highlight", &at_spec_bound)],
            1,
        ),
        (
            "a highlight tag of exactly the tag bound",
            &[("q", "hamlet"), ("highlight", &at_tag_bound)],
            1,
        ),
        (
            "a size of exactly the paging bound",
            &[("q", "hamlet"), ("size", &at_page_bound)],
            1,
        ),
        (
            "a facet size of exactly the bucket bound",
            &[("q", "hamlet"), ("facet", &at_bucket_bound)],
            1,
        ),
        (
            "a sort naming exactly the field bound",
            &[("q", "hamlet"), ("sort", &at_sort_bound)],
            1,
        ),
        (
            "a return naming exactly the field bound",
            &[("q", "hamlet"), ("return", &at_return_bound)],
            1,
        ),
    ];
    for (case, params, found) in accepted {
        let (status, body) = search(&router, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            body["hits"]["found"].as_i64(),
            Some(*found),
            "case {case:?}: expected {found} matches, got {body}"
        );
    }

    // Suggest reads the same caller-controlled query string, under the same bound.
    let (status, body) = suggest(
        &router,
        &[("suggester", "title"), ("q", &past_length_bound)],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "SearchException"),
        "case \"a suggest q past the length bound\": got {body}"
    );
    let (status, body) =
        suggest(&router, &[("suggester", "title"), ("q", &at_length_bound)]).await;
    assert_eq!(
        (status, body["suggest"]["found"].as_i64()),
        (200, Some(0)),
        "case \"a suggest q of exactly the length bound\": got {body}"
    );
    // Suggest's page is bounded by the same documented paging depth as Search's.
    let (status, body) = suggest(
        &router,
        &[("suggester", "title"), ("q", "ha"), ("size", &past_page_bound)],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "SearchException"),
        "case \"a suggest size one past the paging bound\": got {body}"
    );
    let (status, body) = suggest(
        &router,
        &[("suggester", "title"), ("q", "ha"), ("size", &at_page_bound)],
    )
    .await;
    assert_eq!(
        (status, body["suggest"]["found"].as_i64()),
        (200, Some(1)),
        "case \"a suggest size of exactly the paging bound\": got {body}"
    );
}

/// `count` SDF add entries, with ids `d<first>` upwards.
fn batch_of(count: usize, first: usize) -> String {
    let mut body = String::from("[");
    for id in first..first + count {
        if id != first {
            body.push(',');
        }
        body.push_str(&format!(
            r#"{{"type":"add","id":"d{id}","fields":{{"title":"t{id}"}}}}"#
        ));
    }
    body.push(']');
    body
}

/// The store this process holds is bounded: a batch that would leave a domain
/// holding more than [`MAX_DOCUMENTS_PER_DOMAIN`] documents is refused with the
/// modelled `DocumentServiceException` and applies NOTHING, while a batch that
/// only replaces or deletes is judged on what the index would hold afterwards.
#[tokio::test]
async fn a_batch_past_the_document_ceiling_is_refused_and_applies_nothing() {
    let router = router();

    let (status, body) = upload(
        &router,
        "application/json",
        &batch_of(MAX_DOCUMENTS_PER_DOMAIN + 1, 0),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DocumentServiceException"),
        "case \"a batch one document past the ceiling\": got {body}"
    );
    let (_, body) = search(
        &router,
        &[("q", "matchall"), ("q.parser", "structured"), ("size", "1")],
    )
    .await;
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(0),
        "case \"a batch one document past the ceiling\": nothing may be applied, got {body}"
    );

    // Exactly the ceiling is accepted, and the count is computed from the index.
    let (status, body) = upload(
        &router,
        "application/json",
        &batch_of(MAX_DOCUMENTS_PER_DOMAIN, 0),
    )
    .await;
    assert_eq!(
        (status, body["adds"].as_i64()),
        (200, Some(i64::try_from(MAX_DOCUMENTS_PER_DOMAIN).unwrap())),
        "case \"a batch of exactly the ceiling\": got {body}"
    );
    let (_, body) = search(
        &router,
        &[("q", "matchall"), ("q.parser", "structured"), ("size", "1")],
    )
    .await;
    assert_eq!(
        body["hits"]["found"].as_i64(),
        Some(i64::try_from(MAX_DOCUMENTS_PER_DOMAIN).unwrap()),
        "case \"a batch of exactly the ceiling\": got {body}"
    );

    // A batch at the ceiling that only REPLACES documents is accepted …
    let (status, body) = upload(&router, "application/json", &batch_of(10, 0)).await;
    assert_eq!(
        (status, body["adds"].as_i64()),
        (200, Some(10)),
        "case \"a batch that replaces documents at the ceiling\": got {body}"
    );
    // … a batch that would add one more document is not …
    let (status, body) = upload(
        &router,
        "application/json",
        &batch_of(1, MAX_DOCUMENTS_PER_DOMAIN),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DocumentServiceException"),
        "case \"one new document past the ceiling\": got {body}"
    );
    // … and one that deletes a document and adds another stays at the ceiling.
    let swap = format!(
        r#"[{{"type":"delete","id":"d0"}},
            {{"type":"add","id":"d{MAX_DOCUMENTS_PER_DOMAIN}","fields":{{"title":"t"}}}}]"#
    );
    let (status, body) = upload(&router, "application/json", &swap).await;
    assert_eq!(
        (status, body["adds"].as_i64(), body["deletes"].as_i64()),
        (200, Some(1), Some(1)),
        "case \"a delete and an add at the ceiling\": got {body}"
    );
}

/// The number of indexes is bounded too: an index is created per domain endpoint
/// a batch addresses, and the endpoint comes from the request, so a batch for a
/// new domain past [`MAX_DOMAINS`] is refused while the domains already served
/// keep working.
#[tokio::test]
async fn a_new_domain_past_the_domain_ceiling_is_refused() {
    let router = router();
    let endpoint = |domain: usize| {
        format!("doc-d{domain}-1a2b3c.us-east-1.cloudsearchdomain.amazonaws.com")
    };
    let batch = r#"[{"type":"add","id":"d1","fields":{"title":"t"}}]"#;

    for domain in 0..MAX_DOMAINS {
        let host = endpoint(domain);
        let (status, body) = upload_to(&router, &host, "application/json", batch).await;
        assert_eq!(
            (status, body["adds"].as_i64()),
            (200, Some(1)),
            "case \"domain {domain} of {MAX_DOMAINS}\": got {status} {body}"
        );
    }

    let host = endpoint(MAX_DOMAINS);
    let (status, body) = upload_to(&router, &host, "application/json", batch).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DocumentServiceException"),
        "case \"one domain past the ceiling\": got {body}"
    );

    // A domain already in the store still accepts batches.
    let host = endpoint(0);
    let (status, body) = upload_to(&router, &host, "application/json", batch).await;
    assert_eq!(
        (status, body["adds"].as_i64()),
        (200, Some(1)),
        "case \"a domain already served, after the ceiling was hit\": got {body}"
    );
}

/// What `highlight` marks is bounded by the request's `max_phrases`, however many
/// times a term occurs in the value — the number of occurrences is a property of
/// the stored document, not of the request, so the work and the memory one
/// highlight costs must follow `max_phrases` rather than the length of the value.
///
/// The rows differ along `max_phrases` (absent, which documents "the first
/// occurrence is highlighted", through the published maximum) over a value
/// holding one occurrence per byte, which is the worst case the query syntax can
/// ask for: a single-character term.
#[tokio::test]
async fn highlight_marks_at_most_max_phrases_occurrences_however_long_the_value() {
    let router = router();
    // One occurrence of the term `a` every other byte, kept well inside
    // MAX_DOCUMENT_BYTES: 100_000 separate `a` tokens for the query to match and
    // 100_000 occurrences for the highlighter to find.
    let value = "a ".repeat(100_000);
    let batch = format!(r#"[{{"type":"add","id":"long","fields":{{"title":"{value}"}}}}]"#);
    let (status, body) = upload(&router, "application/json", &batch).await;
    assert_eq!(
        (status, body["adds"].as_i64()),
        (200, Some(1)),
        "case \"a document with one occurrence per byte\": got {status} {body}"
    );

    // (case, the highlight parameter, how many occurrences must be marked).
    let cases: &[(&str, &str, usize)] = &[
        ("max_phrases left at its default", r#"{"title":{}}"#, 1),
        ("max_phrases of 1", r#"{"title":{"max_phrases":1}}"#, 1),
        ("max_phrases of 7", r#"{"title":{"max_phrases":7}}"#, 7),
        (
            "max_phrases at the published maximum",
            &format!(r#"{{"title":{{"max_phrases":{MAX_HIGHLIGHT_PHRASES}}}}}"#),
            MAX_HIGHLIGHT_PHRASES,
        ),
    ];
    for (case, highlight, expected) in cases {
        let (status, body) = search(&router, &[("q", "a"), ("highlight", highlight)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let marked = body["hits"]["hit"][0]["highlights"]["title"]
            .as_str()
            .unwrap_or_default()
            .matches("<em>")
            .count();
        assert_eq!(
            marked, *expected,
            "case {case:?}: expected {expected} marked occurrences, got {marked}"
        );
    }

    // A max_phrases that marks nothing, and one past the published maximum, are
    // both refused rather than silently clamped.
    let refused: &[(&str, String)] = &[
        (
            "max_phrases of 0",
            r#"{"title":{"max_phrases":0}}"#.to_string(),
        ),
        (
            "max_phrases one past the maximum",
            format!(
                r#"{{"title":{{"max_phrases":{}}}}}"#,
                MAX_HIGHLIGHT_PHRASES + 1
            ),
        ),
    ];
    for (case, highlight) in refused {
        let (status, body) = search(&router, &[("q", "a"), ("highlight", highlight)]).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "SearchException"),
            "case {case:?}: got {status} {body}"
        );
    }
}
