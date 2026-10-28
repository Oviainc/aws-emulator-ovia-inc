// SPDX-License-Identifier: CC0-1.0

//! The ONE serving rule both document planes of this service answer a fetch under.
//!
//! This crate publishes two document planes — the report/term documents the URL
//! `GetReport` and `GetTermForReport` announce, and the exported document the URL
//! `ExportComplianceInquiry` announces — and both are served by one walk. The rule pinned
//! here is that walk, over a SET of published documents rather than any one of them:
//!
//! 1. a read (`GET`, `HEAD`) is served and any other method is refused `405`, naming the
//!    methods the document is served by in `Allow`;
//! 2. a fetch presenting no authorisation the plane accepts is refused `403 AccessDenied`
//!    with this service's one invalid-token text, BEFORE any existence check — so the same
//!    unauthorised fetch of a path naming no document is still `403`, never `404`;
//! 3. an authorised fetch of an object the plane does not publish is `404 NoSuchKey`;
//! 4. an authorised `GET` serves the stored bytes under the document's own content type
//!    and length, and an authorised `HEAD` reports that same metadata without the bytes;
//! 5. and a fetch changes nothing: the next one serves the same document.
//!
//! The rows span both planes and what each generalises over — document kind (a report
//! document, the term governing it, an exported inquiry), the report version, and the
//! receipt parameter the plane reads — so a rule that held on only one plane fails here.
//! Every case goes through the router: claiming, routing, the handler, the persisted state
//! and serialization, which is the path a client takes.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{a_report_requiring_acceptance, call, emulator, get};

/// The endpoint the harness addresses the emulator on, and therefore the endpoint every
/// URL it is announced resolves at.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// The wire code a document plane refuses an unauthorised fetch with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The wire code a document plane reports an object it does not publish with.
const NO_SUCH_KEY: &str = "NoSuchKey";

/// The wire code a document plane refuses a method it does not serve with.
const METHOD_NOT_ALLOWED: &str = "MethodNotAllowed";

/// This service's one invalid-token text (`crate::state`'s `INVALID_TOKEN_MESSAGE`),
/// spelled out here so a reworded refusal fails this suite rather than being read back out
/// of the code under test.
const INVALID_TOKEN_MESSAGE: &str = "The provided token is invalid.";

/// The text a document plane reports an absent document with (`crate::service`'s
/// `DOCUMENT_NOT_FOUND_MESSAGE`), spelled out for the same reason.
const DOCUMENT_NOT_FOUND_MESSAGE: &str = "The requested document does not exist";

/// The methods a document is served by, as the `405` must state them (RFC 9110).
const DOCUMENT_READ_METHODS: &str = "GET, HEAD";

/// The query parameter a report or term document URL carries its term grant in
/// (`crate::state`'s `TERM_TOKEN_PARAM`).
const TERM_TOKEN_PARAM: &str = "termToken";

/// The query parameter an export document URL carries its download token in
/// (`crate::state`'s `INQUIRY_EXPORT_TOKEN_PARAM`).
const EXPORT_TOKEN_PARAM: &str = "exportToken";

/// One published document, as the caller the URL was announced to holds it.
struct Published {
    /// Which plane and which document of it: the case every assertion names.
    case: String,
    /// The path the document is published at, without the query string.
    path: String,
    /// The query string the announced URL carried: the receipt that authorises the fetch.
    authorisation: String,
    /// The content type this plane serves the document under.
    content_type: &'static str,
    /// Query strings presenting no authorisation this plane accepts, each with its label.
    unauthorised: Vec<(String, String)>,
}

impl Published {
    /// The request URI a client follows the announced URL with.
    fn authorised_uri(&self) -> String {
        format!("{}?{}", self.path, self.authorisation)
    }

    /// The same path with its last segment — the object — replaced by one this plane
    /// publishes nothing at, so an authorised fetch of it is an absent document.
    fn absent_path(&self) -> String {
        let (parent, object) = self
            .path
            .rsplit_once('/')
            .unwrap_or_else(|| panic!("a document path names an object, got {:?}", self.path));
        format!("{parent}/not-published-{object}")
    }

    /// That absent path, under the authorisation the URL carried.
    fn absent_uri(&self) -> String {
        format!("{}?{}", self.absent_path(), self.authorisation)
    }
}

/// What a fetch of a document URI answered.
struct Fetched {
    status: u16,
    content_type: String,
    /// The `Content-Length` the answer reported, which a `HEAD` reports without a body.
    content_length: String,
    /// The `Allow` header a `405` states the served methods in.
    allow: String,
    /// The `x-amzn-errortype` a refusal carries.
    error_type: String,
    body: Bytes,
}

impl Fetched {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Follow a document URI exactly as a client follows a URL it was handed: under `method`,
/// addressed to the endpoint the URL names, and with NO credential of any kind.
async fn fetch(router: &MultiServiceRouter, method: &str, uri: &str) -> Fetched {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", ENDPOINT)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let content_length = header("content-length");
    let allow = header("allow");
    let error_type = header("x-amzn-errortype");
    Fetched {
        status,
        content_type,
        content_length,
        allow,
        error_type,
        body: response.into_body(),
    }
}

/// The path and query of an announced URL, as a client follows it.
fn followed(url: &str) -> (String, String) {
    let prefix = format!("https://{ENDPOINT}");
    let rest = url
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("an announced URL must name {prefix:?}, got {url:?}"));
    let (path, query) = rest
        .split_once('?')
        .unwrap_or_else(|| panic!("an announced URL must carry its receipt, got {url:?}"));
    assert!(
        !query.is_empty(),
        "an announced URL must carry a receipt that authorises the fetch, got {url:?}"
    );
    (path.to_string(), query.to_string())
}

/// The ways a fetch can present no authorisation a plane accepts, as a table: no query
/// string, an empty receipt, a receipt nobody was issued, and the receipt under the SIBLING
/// plane's parameter — which is the one thing that differs between the two planes, so each
/// must read its own.
fn unauthorised(param: &str, sibling: &str, token: &str) -> Vec<(String, String)> {
    vec![
        ("no query string at all".to_string(), String::new()),
        (format!("an empty {param}"), format!("{param}=")),
        (
            format!("a {param} nobody was issued"),
            format!("{param}=never-issued-receipt"),
        ),
        (
            format!("the receipt under the sibling plane's {sibling}"),
            format!("{sibling}={token}"),
        ),
    ]
}

/// Every published document this suite drives the shared walk over: the report/term plane
/// reached through `GetTermForReport` and `GetReport`, and the export plane reached through
/// `CreateComplianceInquiry` and `ExportComplianceInquiry`.
async fn published(router: &MultiServiceRouter) -> Vec<Published> {
    let mut rows = Vec::new();

    // The report plane, on a report whose `acceptanceType` requires the explicit
    // acceptance of its term: that is the report a term grant is the receipt for, so the
    // authorisation step of the walk is the one being asserted. The rows are its first and
    // its newest published version.
    let report = a_report_requiring_acceptance();
    let mut versions = vec![
        report.versions.first().copied().unwrap_or(1),
        report.latest(),
    ];
    versions.dedup();
    for version in versions {
        let term = get(
            router,
            &format!(
                "/v1/report/getTermForReport?reportId={}&reportVersion={version}",
                report.id
            ),
        )
        .await;
        assert_eq!(
            term.status, 200,
            "GetTermForReport on {} v{version} must be served, got {} with {:?}",
            report.id, term.status, term.body
        );
        let token = term.body["termToken"]
            .as_str()
            .unwrap_or_else(|| panic!("GetTermForReport carries a termToken: {:?}", term.body))
            .to_string();
        let (term_path, term_query) = followed(
            term.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("GetTermForReport announces a URL: {:?}", term.body)),
        );
        rows.push(Published {
            case: format!("the term document of {} v{version}", report.id),
            path: term_path,
            authorisation: term_query,
            // A `.txt` document is `text/plain`, by the extension mapping in
            // `src/state.rs` (`content_type_of`).
            content_type: "text/plain",
            unauthorised: unauthorised(TERM_TOKEN_PARAM, EXPORT_TOKEN_PARAM, &token),
        });

        let download = get(
            router,
            &format!(
                "/v1/report/get?reportId={}&reportVersion={version}&termToken={token}",
                report.id
            ),
        )
        .await;
        assert_eq!(
            download.status, 200,
            "GetReport on {} v{version} must be served under its term grant, got {} with {:?}",
            report.id, download.status, download.body
        );
        let (report_path, report_query) = followed(
            download.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("GetReport announces a URL: {:?}", download.body)),
        );
        rows.push(Published {
            case: format!("the report document of {} v{version}", report.id),
            path: report_path,
            authorisation: report_query,
            // A `.pdf` document is `application/pdf`, by the same mapping.
            content_type: "application/pdf",
            unauthorised: unauthorised(TERM_TOKEN_PARAM, EXPORT_TOKEN_PARAM, &token),
        });
    }

    // And the export plane, on a document exported from an inquiry this suite creates.
    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(
            json!({
                "name": "Document plane",
                "inquiryContent": { "query": "Where is customer data stored?" }
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let inquiry_id = created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries an inquiry id: {:?}", created.body))
        .to_string();
    let exported = call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        Bytes::from(json!({ "complianceInquiryId": inquiry_id }).to_string()),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "the export must be served, got {} with {:?}",
        exported.status, exported.body
    );
    let (export_path, export_query) = followed(
        exported.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "ExportComplianceInquiry announces a URL: {:?}",
                    exported.body
                )
            }),
    );
    let export_token = export_query
        .strip_prefix(&format!("{EXPORT_TOKEN_PARAM}="))
        .unwrap_or_else(|| panic!("the export URL carries its {EXPORT_TOKEN_PARAM}"))
        .to_string();
    rows.push(Published {
        case: format!("the exported document of {inquiry_id}"),
        path: export_path,
        authorisation: export_query,
        // `crate::state`'s `INQUIRY_EXPORT_CONTENT_TYPE`.
        content_type: "application/json",
        unauthorised: unauthorised(EXPORT_TOKEN_PARAM, TERM_TOKEN_PARAM, &export_token),
    });

    rows
}

#[tokio::test]
async fn both_document_planes_serve_a_fetch_under_one_rule() {
    let router = emulator();
    let rows = published(&router).await;
    assert!(
        rows.len() >= 3,
        "the rows must span both document planes, got {} of them",
        rows.len()
    );

    for row in &rows {
        let case = &row.case;

        // 1. A read is served, so the authorised GET answers the stored document under its
        //    own content type and length.
        let served = fetch(&router, "GET", &row.authorised_uri()).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: the announced URL must serve its document, got {} with {:?}",
            served.status,
            served.text()
        );
        assert_eq!(
            served.content_type, row.content_type,
            "case {case:?}: expected content type {:?}, got {:?}",
            row.content_type, served.content_type
        );
        assert!(
            !served.body.is_empty(),
            "case {case:?}: the served document must carry content"
        );
        assert_eq!(
            served.content_length,
            served.body.len().to_string(),
            "case {case:?}: expected Content-Length {}, got {:?}",
            served.body.len(),
            served.content_length
        );

        // A HEAD reports that same metadata without the bytes.
        let head = fetch(&router, "HEAD", &row.authorised_uri()).await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: a HEAD of the announced URL must be answered, got {} with {:?}",
            head.status,
            head.text()
        );
        assert_eq!(
            head.content_type, served.content_type,
            "case {case:?}: a HEAD reports the document's own content type, got {:?}",
            head.content_type
        );
        assert_eq!(
            head.content_length, served.content_length,
            "case {case:?}: a HEAD reports the document's own length, got {:?}",
            head.content_length
        );
        assert!(
            head.body.is_empty(),
            "case {case:?}: a HEAD carries no body, got {} bytes",
            head.body.len()
        );

        // And any other method is refused as the method it is, naming the served ones.
        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            let refused = fetch(&router, method, &row.authorised_uri()).await;
            let case = format!("{case}: {method}");
            assert_eq!(
                refused.status, 405,
                "case {case:?}: a document is read-only, got {} with {:?}",
                refused.status,
                refused.text()
            );
            assert_eq!(
                refused.allow, DOCUMENT_READ_METHODS,
                "case {case:?}: expected Allow: {DOCUMENT_READ_METHODS}, got {:?}",
                refused.allow
            );
            assert_eq!(
                refused.error_type, METHOD_NOT_ALLOWED,
                "case {case:?}: expected the {METHOD_NOT_ALLOWED:?} error type, got {:?}",
                refused.error_type
            );
            assert_ne!(
                refused.body, served.body,
                "case {case:?}: a refused method must serve no document"
            );
        }

        // 2. A fetch presenting no authorisation this plane accepts is refused 403, under
        //    both read methods, and BEFORE any existence check — so the same fetch of a
        //    path naming no document is refused the same way rather than reported absent.
        for (label, query) in &row.unauthorised {
            for path in [&row.path, &row.absent_path()] {
                for method in ["GET", "HEAD"] {
                    let uri = if query.is_empty() {
                        path.clone()
                    } else {
                        format!("{path}?{query}")
                    };
                    let answer = fetch(&router, method, &uri).await;
                    let case = format!("{case}: {label} ({method}) at {path}");
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: an unauthorised fetch must be refused 403, got {} \
                         with {:?}",
                        answer.status,
                        answer.text()
                    );
                    assert_eq!(
                        answer.error_type, ACCESS_DENIED,
                        "case {case:?}: expected the {ACCESS_DENIED:?} error type, got {:?}",
                        answer.error_type
                    );
                    if method == "GET" {
                        let body = answer.json();
                        assert_eq!(
                            body["code"].as_str(),
                            Some(ACCESS_DENIED),
                            "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                            body["code"]
                        );
                        assert_eq!(
                            body["message"].as_str(),
                            Some(INVALID_TOKEN_MESSAGE),
                            "case {case:?}: expected this service's invalid-token text, got {:?}",
                            body["message"]
                        );
                    }
                    assert_ne!(
                        answer.body, served.body,
                        "case {case:?}: a refused fetch must serve no document"
                    );
                }
            }
        }

        // 3. An authorised fetch of an object the plane does not publish is reported
        //    absent, in a message carrying nothing of the request.
        let absent = fetch(&router, "GET", &row.absent_uri()).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: an object the plane does not publish must be 404, got {} with {:?}",
            absent.status,
            absent.text()
        );
        assert_eq!(
            absent.error_type, NO_SUCH_KEY,
            "case {case:?}: expected the {NO_SUCH_KEY:?} error type, got {:?}",
            absent.error_type
        );
        let absent_body = absent.json();
        assert_eq!(
            absent_body["message"].as_str(),
            Some(DOCUMENT_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the absent-document text, got {:?}",
            absent_body["message"]
        );

        // 4. And none of that changed what a later authorised fetch sees: a fetch is a
        //    read, so the document and the receipt that authorises it both survive it.
        let again = fetch(&router, "GET", &row.authorised_uri()).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: the authorised fetch must still be served, got {} with {:?}",
            again.status,
            again.text()
        );
        assert_eq!(
            again.body, served.body,
            "case {case:?}: two fetches of one URL must serve the same document"
        );
    }
}
