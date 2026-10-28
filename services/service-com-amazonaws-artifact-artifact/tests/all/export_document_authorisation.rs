// SPDX-License-Identifier: CC0-1.0

//! What authorises a fetch of a compliance-inquiry EXPORT document.
//!
//! ONE rule is pinned here, over a SET of fetches rather than any one of them: **the
//! document a presigned URL announces is served only to a fetch presenting the
//! authorisation that URL carries, and a fetch presenting none, or one that does not
//! authorise the document the path names, is refused `403 AccessDenied` before any
//! document is looked up.** It is the rule the report and term plane already answers under
//! (`document_data_plane.rs` covers its `termToken` refusals), asked of the export plane:
//! both planes are reached unsigned — that is what a presigned URL is for — so the token
//! the URL carries is the only thing that distinguishes the caller the URL was handed to
//! from everyone else.
//!
//! The ids the path carries authorise nothing, and this suite says so twice: they are
//! minted from counters, so a caller that never called the control plane can derive the
//! whole path, and deriving it still answers `403`.
//!
//! Every assertion goes through the router, so each case exercises claiming, routing, the
//! handler, the persisted state and serialization — the same path a client takes.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator};

/// The path prefix the export documents are published under (`crate::state`'s
/// `EXPORT_DOCUMENT_PATH_PREFIX`), spelled out here so a change to it has to be made
/// deliberately in both places.
const EXPORT_DOCUMENT_PATH_PREFIX: &str = "/v1/compliance-inquiry/download";

/// The file name an export document is published under (`crate::state`'s
/// `INQUIRY_EXPORT_OBJECT`), spelled out for the same reason.
const INQUIRY_EXPORT_OBJECT: &str = "compliance-inquiry-report.json";

/// The query parameter the announced URL carries its download token in.
const EXPORT_TOKEN_PARAM: &str = "exportToken";

/// The endpoint the harness's signed sender addresses the emulator on, which is the
/// endpoint an announced URL must therefore name.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// The wire code a document-plane refusal of an unauthorised fetch carries.
const ACCESS_DENIED: &str = "AccessDenied";

/// This service's one invalid-token text (`crate::state`'s `INVALID_TOKEN_MESSAGE`),
/// spelled out here so a reworded refusal fails this suite rather than being read back out
/// of the code under test.
const INVALID_TOKEN_MESSAGE: &str = "The provided token is invalid.";

/// One announced export URL taken apart: what the caller it was handed to holds.
struct Announced {
    /// The inquiry the export was taken from.
    inquiry_id: String,
    /// The export the URL names.
    export_id: String,
    /// The path the document is published at, without the query string.
    path: String,
    /// The download token the URL carries — the one thing that authorises the fetch.
    token: String,
}

impl Announced {
    /// The request URI a client follows this URL with: the document's path, carrying the
    /// token the URL was announced with.
    fn authorised_uri(&self) -> String {
        format!("{}?{EXPORT_TOKEN_PARAM}={}", self.path, self.token)
    }
}

/// Take one announced URL apart, asserting it names this caller's endpoint and carries a
/// token at all.
fn announced(url: &str, inquiry_id: &str) -> Announced {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("{url:?} is not an absolute URL"));
    let (host, path_and_query) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("{url:?} carries no path"));
    assert_eq!(
        host, ENDPOINT,
        "the announced URL must name the endpoint the caller reached, got {url:?}"
    );
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or_else(|| panic!("{url:?} must carry the token that authorises the fetch"));
    let token = query
        .strip_prefix(&format!("{EXPORT_TOKEN_PARAM}="))
        .unwrap_or_else(|| panic!("{url:?} must carry its {EXPORT_TOKEN_PARAM}"))
        .to_string();
    assert!(
        !token.is_empty(),
        "the announced URL must carry a token, got {url:?}"
    );
    let path = format!("/{path}");
    assert!(
        path.ends_with(INQUIRY_EXPORT_OBJECT),
        "the announced URL must name the export object, got {url:?}"
    );
    let export_id = path
        .rsplit('/')
        .nth(1)
        .unwrap_or_else(|| panic!("{url:?} names no export id"))
        .to_string();
    Announced {
        inquiry_id: inquiry_id.to_string(),
        export_id,
        path,
        token,
    }
}

/// What an unsigned fetch of a document URI answered.
struct Fetched {
    status: u16,
    /// The `x-amzn-errortype` header a refusal carries.
    error_type: String,
    body: Vec<u8>,
}

impl Fetched {
    /// The answered body as JSON, or `Value::Null` when it is not JSON (a `HEAD` carries
    /// no body at all).
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The answered body as text, for a message that must carry nothing of the request.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Follow a document URI exactly as a client follows a presigned URL: under `method`,
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Fetched {
        status,
        error_type,
        body: response.into_body().to_vec(),
    }
}

/// `CreateComplianceInquiry` of one submitted query, answering the inquiry id it minted.
async fn create_inquiry(router: &MultiServiceRouter, name: &str, query: &str) -> String {
    let body = json!({ "name": name, "inquiryContent": { "query": query } });
    let answer = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "the create must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries no inquiry id: {:?}", answer.body))
        .to_string()
}

/// `ExportComplianceInquiry` of one inquiry, answering the URL it announced, taken apart.
async fn export_inquiry(router: &MultiServiceRouter, inquiry_id: &str) -> Announced {
    let body = json!({ "complianceInquiryId": inquiry_id });
    let answer = call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the export must be served, got {} with {:?}",
        answer.status, answer.body
    );
    let url = answer.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("the export carries no document URL: {:?}", answer.body))
        .to_string();
    announced(&url, inquiry_id)
}

/// The token with its last character dropped, which is a token this service did not mint.
fn truncated(token: &str) -> String {
    let kept = token.chars().count().saturating_sub(1);
    token.chars().take(kept).collect()
}

#[tokio::test]
async fn only_the_token_an_export_url_carries_authorises_its_document() {
    let router = emulator();
    let first_query = "Where is customer data stored?";
    let first = create_inquiry(&router, "Data residency", first_query).await;
    let second = create_inquiry(&router, "Encryption", "Is customer data encrypted at rest?").await;
    let export = export_inquiry(&router, &first).await;
    // A second export of the SAME inquiry, and an export of another inquiry: each is its
    // own document, so each is authorised by its own token and by no other.
    let sibling = export_inquiry(&router, &first).await;
    let other = export_inquiry(&router, &second).await;
    assert_ne!(
        export.token, sibling.token,
        "two exports of one inquiry are two documents, so each must carry its own token, \
         got {:?} twice",
        export.token
    );

    // One row per way a fetch can fail to present the authorisation the URL carries: no
    // query string at all, an empty token, the token under the parameter the sibling
    // plane reads, a token this service never minted, a counter-shaped guess, each id the
    // path itself carries, the token mangled three ways, and the token of another export
    // — of the same inquiry and of another.
    let unauthorised = vec![
        ("no query string at all", String::new()),
        ("an empty token", format!("{EXPORT_TOKEN_PARAM}=")),
        (
            "the token under the report plane's parameter",
            format!("termToken={}", export.token),
        ),
        (
            "a token this service never minted",
            format!("{EXPORT_TOKEN_PARAM}=00000000000000000000000000000000"),
        ),
        (
            "a counter-shaped guess",
            format!("{EXPORT_TOKEN_PARAM}=export-0000000000000001"),
        ),
        (
            "the export id as the token",
            format!("{EXPORT_TOKEN_PARAM}={}", export.export_id),
        ),
        (
            "the inquiry id as the token",
            format!("{EXPORT_TOKEN_PARAM}={}", export.inquiry_id),
        ),
        (
            "the token truncated",
            format!("{EXPORT_TOKEN_PARAM}={}", truncated(&export.token)),
        ),
        (
            "the token in upper case",
            format!("{EXPORT_TOKEN_PARAM}={}", export.token.to_uppercase()),
        ),
        (
            "the token with a digit appended",
            format!("{EXPORT_TOKEN_PARAM}={}0", export.token),
        ),
        (
            "another export of the same inquiry's token",
            format!("{EXPORT_TOKEN_PARAM}={}", sibling.token),
        ),
        (
            "another inquiry's export token",
            format!("{EXPORT_TOKEN_PARAM}={}", other.token),
        ),
    ];

    for (label, query) in unauthorised {
        // Under both read methods, because the refusal is of the fetch and not of the
        // method: a `HEAD` is a fetch of the same document.
        for method in ["GET", "HEAD"] {
            let uri = if query.is_empty() {
                export.path.clone()
            } else {
                format!("{}?{query}", export.path)
            };
            let answer = fetch(&router, method, &uri).await;
            let case = format!("{label} ({method})");
            assert_eq!(
                answer.status, 403,
                "case {case:?}: an unauthorised fetch must be refused 403, got {} with {:?}",
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
            // The refusal states nothing of the request and nothing of the document: it
            // neither confirms which ids exist nor discloses what the export holds.
            let text = answer.text();
            for leaked in [
                export.inquiry_id.as_str(),
                export.export_id.as_str(),
                first_query,
            ] {
                assert!(
                    !text.contains(leaked),
                    "case {case:?}: the refusal must carry nothing from the request or the \
                     document, yet {leaked:?} is in {text:?}"
                );
            }
        }
    }

    // And the fetch each URL does authorise is served: every export under its OWN token,
    // whatever else the query string carries beside it — the token is read by name.
    for (label, handed) in [
        ("the first export", &export),
        ("a second export of one inquiry", &sibling),
        ("an export of another inquiry", &other),
    ] {
        for (how, uri) in [
            ("the announced URL", handed.authorised_uri()),
            (
                "the token beside another parameter",
                format!(
                    "{}?response-content-type=application%2Fjson&{EXPORT_TOKEN_PARAM}={}",
                    handed.path, handed.token
                ),
            ),
        ] {
            let served = fetch(&router, "GET", &uri).await;
            let case = format!("{label}: {how}");
            assert_eq!(
                served.status, 200,
                "case {case:?}: the authorised fetch must serve the document, got {} with {:?}",
                served.status,
                served.text()
            );
            let document = served.json();
            assert_eq!(
                document["complianceInquiryId"].as_str(),
                Some(handed.inquiry_id.as_str()),
                "case {case:?}: the document must report the inquiry it was exported from, got \
                 {:?}",
                document["complianceInquiryId"]
            );
        }
    }
}

#[tokio::test]
async fn a_refused_fetch_changes_nothing_an_authorised_one_sees() {
    let router = emulator();
    let id = create_inquiry(&router, "Retention", "How long are access logs retained?").await;
    let handed = export_inquiry(&router, &id).await;

    let before = fetch(&router, "GET", &handed.authorised_uri()).await;
    assert_eq!(
        before.status, 200,
        "the announced URL must serve the document, got {} with {:?}",
        before.status,
        before.text()
    );

    for attempt in 1..=3 {
        let refused = fetch(&router, "GET", &handed.path).await;
        assert_eq!(
            refused.status, 403,
            "attempt {attempt}: a fetch carrying no token must be refused, got {} with {:?}",
            refused.status,
            refused.text()
        );
    }

    let after = fetch(&router, "GET", &handed.authorised_uri()).await;
    assert_eq!(
        after.status, 200,
        "the authorised fetch must still be served after a refusal, got {} with {:?}",
        after.status,
        after.text()
    );
    assert_eq!(
        after.body, before.body,
        "a refused fetch must change nothing a later authorised one sees, got {:?} against \
         {:?}",
        after.text(),
        before.text()
    );
}

/// The path of the `nth_export`-th export of the `nth_inquiry`-th inquiry, as the counters
/// this emulator mints its ids from make it derivable: a caller that never called the
/// control plane can write this down.
fn derivable_path(nth_inquiry: u64, nth_export: u64) -> String {
    format!(
        "{EXPORT_DOCUMENT_PATH_PREFIX}/compliance-inquiry-{nth_inquiry:016x}/export/\
         export-{nth_export:016x}/{INQUIRY_EXPORT_OBJECT}"
    )
}

#[tokio::test]
async fn a_derivable_document_path_authorises_nobody() {
    let router = emulator();

    // Before anything is minted: a derived path names no export, and the fetch presents no
    // token, so it is refused rather than reported absent.
    for (nth_inquiry, nth_export) in [(1, 1), (2, 2)] {
        let path = derivable_path(nth_inquiry, nth_export);
        let answer = fetch(&router, "GET", &path).await;
        assert_eq!(
            answer.status, 403,
            "case {path:?}: a derived path must authorise nothing, got {} with {:?}",
            answer.status,
            answer.text()
        );
    }

    // And once those very records exist, the same derived path is still refused while the
    // URL the export announced is served — which is what says the path authorises nobody
    // and the token the URL carries is what does.
    let first = create_inquiry(&router, "First", "Where is customer data stored?").await;
    let first_export = export_inquiry(&router, &first).await;
    let second = create_inquiry(&router, "Second", "Is customer data encrypted at rest?").await;
    let second_export = export_inquiry(&router, &second).await;

    for (label, nth, handed) in [
        ("the first export", (1, 1), &first_export),
        ("the second export", (2, 2), &second_export),
    ] {
        let path = derivable_path(nth.0, nth.1);
        assert_eq!(
            handed.path, path,
            "case {label:?}: the minted ids are the derivable ones, which is what this case \
             rests on; got {:?}",
            handed.path
        );
        let refused = fetch(&router, "GET", &path).await;
        assert_eq!(
            refused.status, 403,
            "case {label:?}: the derived path must still authorise nothing, got {} with {:?}",
            refused.status,
            refused.text()
        );
        let served = fetch(&router, "GET", &handed.authorised_uri()).await;
        assert_eq!(
            served.status, 200,
            "case {label:?}: the announced URL must serve the document, got {} with {:?}",
            served.status,
            served.text()
        );
    }
}
