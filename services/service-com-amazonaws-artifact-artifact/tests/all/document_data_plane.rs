// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENT data plane behind the URLs AWS Artifact announces.
//!
//! One rule is pinned here, over a SET of documents rather than any one of them:
//! **an announced document URL resolves.** The `documentPresignedUrl` `GetReport` and
//! `GetTermForReport` answer with is fetchable over plain HTTP and serves the stored
//! document, under the content type that document's file name implies — and only under the
//! receipt that URL carries: a term grant covering the TERM governing that report,
//! whatever the report's `acceptanceType`. Fetching it changes nothing.
//!
//! Every assertion goes through the router, so each case exercises the same path a
//! client takes: routing, the front door, the handler, the persisted state and
//! serialization.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{ACCEPTANCE_EXPLICIT, ACCEPTANCE_PASSTHROUGH};

/// A SigV4 credential scope naming this service: what routes a control-plane request to
/// it.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The endpoint host this service is addressed by.
const HOST: &str = "artifact.eu-west-1.amazonaws.com";

/// One emulator over `engine`, with request authentication on or off.
fn emulator_over(engine: &Arc<engine::InMemoryEngine>, authenticated: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// The usual emulator of these tests: one service, one engine, authentication off.
fn emulator() -> MultiServiceRouter {
    emulator_over(&Arc::new(engine::InMemoryEngine::new()), false)
}

/// What the emulator answered.
struct Answer {
    status: u16,
    content_type: String,
    /// The `Allow` header, which a `405` states the target's read methods in.
    allow: String,
    bytes: Bytes,
    body: Value,
}

/// `GET uri`, signed for this service.
async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, uri, true, HOST).await
}

/// `GET uri` with no `Authorization` header at all — how a caller fetches a URL the
/// service handed it.
async fn get_unsigned(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, uri, false, HOST).await
}

/// `GET uri` addressed to the emulator's own socket rather than to this service's
/// endpoint host, and unsigned: nothing in the request names a service, so only the
/// document path can.
async fn get_from_socket(router: &MultiServiceRouter, uri: &str) -> Answer {
    send(router, uri, false, "127.0.0.1:9297").await
}

/// `GET` the same document in ABSOLUTE form (`https://host/path?query`), the request
/// form an SDK-built request carries, and unsigned.
async fn get_absolute_form(router: &MultiServiceRouter, path_and_query: &str) -> Answer {
    send(
        router,
        &format!("https://{HOST}{path_and_query}"),
        false,
        HOST,
    )
    .await
}

/// `GET uri` signed for this service, addressed to `host`: the endpoint the caller
/// reached the emulator on.
async fn get_addressed_to(router: &MultiServiceRouter, uri: &str, host: &str) -> Answer {
    send(router, uri, true, host).await
}

/// `GET uri` unsigned, addressed to `host`.
async fn get_unsigned_addressed_to(router: &MultiServiceRouter, uri: &str, host: &str) -> Answer {
    send(router, uri, false, host).await
}

async fn send(router: &MultiServiceRouter, uri: &str, signed: bool, host: &str) -> Answer {
    let mut request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", host);
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let request = request
        .body(Bytes::new())
        .expect("the test request is well formed");

    dispatch(router, request).await
}

/// `GET uri` with NO `Host` header at all: an HTTP/2 or SDK-built request carries its
/// endpoint on the URI's authority instead, which is the only place the endpoint can be
/// read from.
async fn send_without_host_header(router: &MultiServiceRouter, uri: &str, signed: bool) -> Answer {
    let mut request = ::http::Request::builder().method("GET").uri(uri);
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let request = request
        .body(Bytes::new())
        .expect("the test request is well formed");

    dispatch(router, request).await
}

async fn dispatch(router: &MultiServiceRouter, request: ::http::Request<Bytes>) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let allow = response
        .headers()
        .get("allow")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        content_type,
        allow,
        bytes,
        body,
    }
}

/// The path and query of a URL the service announced, as a caller fetches it: the host
/// is the service's own endpoint, so what is left to dispatch is everything after it.
fn fetchable(url: &str) -> String {
    let after_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("an announced URL must carry a scheme, got {url:?}"));
    let (host, path) = after_scheme
        .split_once('/')
        .unwrap_or_else(|| panic!("an announced URL must carry a path, got {url:?}"));
    assert_eq!(
        host, HOST,
        "an announced URL must name this service's own endpoint so it resolves, got {url:?}"
    );
    format!("/{path}")
}

/// Every report the catalog publishes, with its published versions — read from the API,
/// so the rows below are whatever this emulator publishes.
async fn catalog(router: &MultiServiceRouter) -> Vec<(String, Vec<i64>)> {
    let listed = get(router, "/v1/report/list").await;
    let reports = listed.body["reports"]
        .as_array()
        .expect("ListReports must report the catalog")
        .clone();
    assert!(
        !reports.is_empty(),
        "the catalog must publish a report to download, got {:?}",
        listed.body
    );
    let mut rows = Vec::new();
    for report in &reports {
        let id = report["id"]
            .as_str()
            .expect("every summary reports its id")
            .to_string();
        let versions = get(router, &format!("/v1/report/listVersions?reportId={id}")).await;
        let numbers = versions.body["reports"]
            .as_array()
            .expect("ListReportVersions must report the versions")
            .iter()
            .map(|summary| {
                summary["version"]
                    .as_i64()
                    .expect("every version reports its number")
            })
            .collect();
        rows.push((id, numbers));
    }
    rows
}

/// The `acceptanceType` the catalog reports for one report — what decides whether
/// downloading it requires the explicit click-through acceptance of its term — read from
/// the API rather than assumed.
async fn acceptance_type(router: &MultiServiceRouter, id: &str) -> String {
    let metadata = get(router, &format!("/v1/report/getMetadata?reportId={id}")).await;
    metadata.body["reportDetails"]["acceptanceType"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("every report reports its acceptanceType, got {:?}", metadata.body)
        })
        .to_string()
}

#[tokio::test]
async fn every_announced_document_url_serves_its_stored_document() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(&engine, false);
    // A second front door over the SAME engine, with request authentication ON: a
    // document URL is fetched with no `Authorization` header at all, so it has to be
    // served unsigned even where the control plane is not.
    let authenticated = emulator_over(&engine, true);

    // The rule generalises over the document, so the rows are every (report, version,
    // kind) the catalog publishes: PDF report documents and plain-text term documents,
    // for reports with one version and reports with several.
    let mut served_documents: Vec<(String, Bytes)> = Vec::new();
    for (id, versions) in catalog(&router).await {
        for version in versions {
            let case = format!("{id} v{version}");

            let term = get(
                &router,
                &format!("/v1/report/getTermForReport?reportId={id}&reportVersion={version}"),
            )
            .await;
            assert_eq!(
                term.status, 200,
                "case {case:?}: expected HTTP 200 from GetTermForReport, got {} with {:?}",
                term.status, term.body
            );
            let token = term.body["termToken"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let term_url = term.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            // The term document the acceptance was served under is fetchable, as text.
            let term_document = get_unsigned(&authenticated, &fetchable(&term_url)).await;
            assert_eq!(
                term_document.status, 200,
                "case {case:?}: the announced term URL must serve a document, got {} with {:?}",
                term_document.status,
                String::from_utf8_lossy(&term_document.bytes)
            );
            assert_eq!(
                term_document.content_type, "text/plain",
                "case {case:?}: a .txt term document is text/plain, got {:?}",
                term_document.content_type
            );
            assert!(
                !term_document.bytes.is_empty(),
                "case {case:?}: the term document must have content"
            );

            // And so is the report document, as the PDF its file name declares.
            let download = get(
                &router,
                &format!("/v1/report/get?reportId={id}&reportVersion={version}&termToken={token}"),
            )
            .await;
            assert_eq!(
                download.status, 200,
                "case {case:?}: expected HTTP 200 from GetReport, got {} with {:?}",
                download.status, download.body
            );
            let report_url = download.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let document = get_unsigned(&authenticated, &fetchable(&report_url)).await;
            assert_eq!(
                document.status, 200,
                "case {case:?}: the announced report URL must serve a document, got {} with {:?}",
                document.status,
                String::from_utf8_lossy(&document.bytes)
            );
            assert_eq!(
                document.content_type, "application/pdf",
                "case {case:?}: a .pdf report document is application/pdf, got {:?}",
                document.content_type
            );
            assert!(
                document.bytes.starts_with(b"%PDF-"),
                "case {case:?}: a PDF document begins with its magic bytes, got {:?}",
                String::from_utf8_lossy(&document.bytes[..document.bytes.len().min(16)])
            );
            // A PDF is a whole document, not a prefix: it ends with the trailer marker
            // every PDF reader looks for first.
            assert!(
                document.bytes.ends_with(b"%%EOF\n") || document.bytes.ends_with(b"%%EOF"),
                "case {case:?}: a PDF document ends with its trailer marker, got {:?}",
                String::from_utf8_lossy(&document.bytes[document.bytes.len().saturating_sub(16)..])
            );

            // Fetching a document changes nothing: the second fetch serves the same
            // bytes.
            let again = get_unsigned(&authenticated, &fetchable(&report_url)).await;
            assert_eq!(
                again.bytes, document.bytes,
                "case {case:?}: two fetches of one URL must serve the same document"
            );

            // And the same fetch addressed to the emulator's socket — where neither a
            // credential nor the host names a service — serves the same document: a
            // client that was handed a URL fetches it however it reaches the emulator.
            let direct = get_from_socket(&authenticated, &fetchable(&report_url)).await;
            assert_eq!(
                direct.bytes, document.bytes,
                "case {case:?}: a document URL must resolve however the emulator is addressed"
            );

            // Including the absolute-form request URI an SDK-built request carries.
            let absolute = get_absolute_form(&authenticated, &fetchable(&report_url)).await;
            assert_eq!(
                absolute.bytes, document.bytes,
                "case {case:?}: a document URL must resolve in absolute request form too"
            );

            served_documents.push((case, document.bytes));
        }
    }

    // Each published version carries its OWN document, so no two of the documents the
    // catalog serves are the same bytes.
    for (index, (case, bytes)) in served_documents.iter().enumerate() {
        for (other_case, other_bytes) in served_documents.iter().skip(index + 1) {
            assert_ne!(
                bytes, other_bytes,
                "case {case:?}: every published version publishes its own document, but it \
                 serves the same bytes as {other_case:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_document_is_served_only_under_a_grant_for_its_term() {
    let router = emulator();
    let rows = catalog(&router).await;
    // The gate this test pins is the one a report requiring the explicit acceptance of
    // its term is served under, so the row is such a report; what a report that requires
    // no acceptance is served under is pinned by the test below.
    let mut gated = None;
    for (candidate, versions) in &rows {
        if acceptance_type(&router, candidate).await == ACCEPTANCE_EXPLICIT {
            gated = Some((candidate.clone(), versions.clone()));
            break;
        }
    }
    let (id, versions) =
        gated.expect("the catalog publishes a report requiring explicit acceptance");
    let id = &id;
    let version = versions.first().copied().unwrap_or(1);

    let term = get(
        &router,
        &format!("/v1/report/getTermForReport?reportId={id}&reportVersion={version}"),
    )
    .await;
    let token = term.body["termToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let download = get(
        &router,
        &format!("/v1/report/get?reportId={id}&reportVersion={version}&termToken={token}"),
    )
    .await;
    let url = fetchable(
        download.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default(),
    );
    let path = url
        .split_once('?')
        .map_or(url.as_str(), |(path, _)| path)
        .to_string();

    // A grant minted for ANOTHER report covers another term document, so it authorises
    // none of this report's documents.
    let other = rows
        .iter()
        .map(|(other, _)| other)
        .find(|other| *other != id)
        .expect("the catalog publishes more than one report");
    let other_term = get(
        &router,
        &format!("/v1/report/getTermForReport?reportId={other}"),
    )
    .await;
    let other_token = other_term.body["termToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The rule generalises over what is wrong with the fetch, so the rows are: no grant
    // at all, a grant nobody was issued, a grant issued under another report's term, a
    // document the report never published, and a path naming no document.
    let cases = [
        ("no termToken", path.clone(), 403),
        (
            "a token nobody was issued",
            format!("{path}?termToken=never-issued"),
            403,
        ),
        (
            "a token issued under another report's term",
            format!("{path}?termToken={other_token}"),
            403,
        ),
        (
            "a document the report never published",
            format!(
                "/v1/report/download/report/{id}/v{version}/not-published.pdf?termToken={token}"
            ),
            404,
        ),
        (
            "a path naming no document",
            format!("/v1/report/download/report/{id}?termToken={token}"),
            // Not a document path at all, so no modelled operation matches it either:
            // the request is refused as unrouted rather than as unauthorised.
            501,
        ),
    ];
    for (case, uri, expected) in cases {
        let answer = get_unsigned(&router, &uri).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.bytes)
        );
        assert!(
            !answer.bytes.starts_with(b"%PDF-"),
            "case {case:?}: a refused fetch must serve no document"
        );
    }

    // And the grant is not consumed by any of that: the authorised fetch still serves.
    let served = get_unsigned(&router, &url).await;
    assert_eq!(
        served.status, 200,
        "an authorised fetch must still be served, got {} with {:?}",
        served.status,
        String::from_utf8_lossy(&served.bytes)
    );
}

#[tokio::test]
async fn a_report_requiring_no_acceptance_still_serves_its_document_only_under_a_grant() {
    let router = emulator();

    // The rule generalises over the report, so the rows are every (report, version) the
    // catalog publishes whose `acceptanceType` requires no explicit acceptance of its
    // term — reports with one version and reports with several. The acceptance type says
    // whether a client has to show the term document to a person, not whether the receipt
    // is needed: with no grant minted, the download is refused; once the term is accepted,
    // the same request is served and its URL resolves.
    let mut covered = 0usize;
    for (id, versions) in catalog(&router).await {
        if acceptance_type(&router, &id).await != ACCEPTANCE_PASSTHROUGH {
            continue;
        }
        for version in versions {
            let case = format!("{id} v{version}");
            let refused = get(
                &router,
                &format!("/v1/report/get?reportId={id}&reportVersion={version}&termToken=unaccepted"),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: a token no acceptance minted must be refused whatever the \
                 acceptance type, got {} with {:?}",
                refused.status, refused.body
            );
            assert!(
                refused.body["documentPresignedUrl"].is_null(),
                "case {case:?}: a refused download must announce no URL, got {:?}",
                refused.body
            );

            let term = get(
                &router,
                &format!("/v1/report/getTermForReport?reportId={id}&reportVersion={version}"),
            )
            .await;
            assert_eq!(
                term.status, 200,
                "case {case:?}: the term must be served, got {} with {:?}",
                term.status, term.body
            );
            let token = term.body["termToken"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let download = get(
                &router,
                &format!(
                    "/v1/report/get?reportId={id}&reportVersion={version}&termToken={token}"
                ),
            )
            .await;
            assert_eq!(
                download.status, 200,
                "case {case:?}: the receipt its own acceptance minted must download the \
                 report, got {} with {:?}",
                download.status, download.body
            );
            let url = fetchable(
                download.body["documentPresignedUrl"]
                    .as_str()
                    .unwrap_or_default(),
            );

            // The URL it announces resolves, which is what announcing one is for.
            let document = get_unsigned(&router, &url).await;
            assert_eq!(
                document.status, 200,
                "case {case:?}: the announced URL must serve its document, got {} with {:?}",
                document.status,
                String::from_utf8_lossy(&document.bytes)
            );
            assert!(
                document.bytes.starts_with(b"%PDF-"),
                "case {case:?}: expected the report document, got {:?}",
                String::from_utf8_lossy(&document.bytes[..document.bytes.len().min(16)])
            );

            // And the data plane applies the same rule: the receipt the URL carries is what
            // authorises the fetch, so the same path without it serves nothing.
            let path = url
                .split_once('?')
                .map_or(url.as_str(), |(path, _)| path)
                .to_string();
            let bare = get_unsigned(&router, &path).await;
            assert_eq!(
                bare.status, 403,
                "case {case:?}: a fetch carrying no receipt must be refused, got {} with {:?}",
                bare.status,
                String::from_utf8_lossy(&bare.bytes)
            );
            assert!(
                !bare.bytes.starts_with(b"%PDF-"),
                "case {case:?}: a refused fetch must serve no document"
            );
            covered += 1;
        }
    }
    assert!(
        covered > 0,
        "the catalog must publish a report requiring no explicit acceptance of its term"
    );
}

#[tokio::test]
async fn an_announced_url_names_the_endpoint_the_caller_reached_the_emulator_on() {
    let router = emulator();
    let rows = catalog(&router).await;
    let (id, _) = rows.first().expect("the catalog publishes a report");

    // The rule generalises over the endpoint a caller addressed, so the rows are
    // endpoints: this service's own, the emulator's socket, and a loopback name. The
    // announced URL must name the one the request carried — never a real AWS host, which
    // would resolve off this machine — and the document must be fetchable there.
    for host in [
        "artifact.eu-west-1.amazonaws.com",
        "127.0.0.1:9297",
        "localhost:9297",
    ] {
        let term = get_addressed_to(
            &router,
            &format!("/v1/report/getTermForReport?reportId={id}"),
            host,
        )
        .await;
        assert_eq!(
            term.status, 200,
            "case {host:?}: expected HTTP 200 from GetTermForReport, got {} with {:?}",
            term.status, term.body
        );
        let token = term.body["termToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let announced = term.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            announced.starts_with(&format!("https://{host}/v1/report/download/")),
            "case {host:?}: the announced URL must name the endpoint the request reached, \
             got {announced:?}"
        );

        let download = get_addressed_to(
            &router,
            &format!("/v1/report/get?reportId={id}&termToken={token}"),
            host,
        )
        .await;
        let report_url = download.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            report_url.starts_with(&format!("https://{host}/v1/report/download/")),
            "case {host:?}: GetReport must announce the same endpoint, got {report_url:?}"
        );

        // And it is fetchable there, which is what naming that endpoint is for.
        let path_and_query = report_url
            .split_once(host)
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default();
        let document = get_unsigned_addressed_to(&router, &path_and_query, host).await;
        assert_eq!(
            document.status, 200,
            "case {host:?}: the announced URL must serve its document at that endpoint, got \
             {} with {:?}",
            document.status,
            String::from_utf8_lossy(&document.bytes)
        );
        assert!(
            document.bytes.starts_with(b"%PDF-"),
            "case {host:?}: expected the report document, got {:?}",
            String::from_utf8_lossy(&document.bytes[..document.bytes.len().min(16)])
        );
    }

    // A `Host` the caller chose is only honoured when it IS an endpoint: anything
    // carrying a path, a query, userinfo or a space is not one, so the URL announces the
    // loopback literal rather than reflecting it back.
    for forged in [
        "elsewhere.example.com/v1/report/download",
        "elsewhere.example.com?x=1",
        "user@elsewhere.example.com",
        "elsewhere example com",
        "",
    ] {
        let term = get_addressed_to(
            &router,
            &format!("/v1/report/getTermForReport?reportId={id}"),
            forged,
        )
        .await;
        let announced = term.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            announced.starts_with("https://127.0.0.1/v1/report/download/"),
            "case {forged:?}: a Host that is not an endpoint must not be announced, got \
             {announced:?}"
        );
    }
}

#[tokio::test]
async fn a_document_url_answers_a_head_and_refuses_another_method() {
    let router = emulator();
    let rows = catalog(&router).await;
    let (id, versions) = rows.first().expect("the catalog publishes a report");
    let version = versions.first().copied().unwrap_or(1);

    let term = get(
        &router,
        &format!("/v1/report/getTermForReport?reportId={id}&reportVersion={version}"),
    )
    .await;
    let token = term.body["termToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let download = get(
        &router,
        &format!("/v1/report/get?reportId={id}&reportVersion={version}&termToken={token}"),
    )
    .await;
    let url = fetchable(
        download.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default(),
    );

    let served = get_unsigned(&router, &url).await;
    assert_eq!(
        served.status, 200,
        "a GET of the announced URL must serve the document, got {}",
        served.status
    );

    // The rule generalises over the method a caller uses on a URL it was handed: a read
    // is answered (HEAD reports the document's metadata without its bytes, under the same
    // length a GET carries), and anything else is refused as the method it is.
    let head = request(&router, "HEAD", &url).await;
    assert_eq!(
        head.status, 200,
        "case HEAD: a read of the announced URL must be answered, got {} with {:?}",
        head.status,
        String::from_utf8_lossy(&head.bytes)
    );
    assert_eq!(
        head.content_type, served.content_type,
        "case HEAD: expected the document's own content type, got {:?}",
        head.content_type
    );
    assert!(
        head.bytes.is_empty(),
        "case HEAD: a HEAD carries no body, got {} bytes",
        head.bytes.len()
    );

    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        let refused = request(&router, method, &url).await;
        assert_eq!(
            refused.status, 405,
            "case {method}: a document URL is read-only, got {} with {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.bytes)
        );
        assert!(
            !refused.bytes.starts_with(b"%PDF-"),
            "case {method}: a refused request must serve no document"
        );
        // RFC 9110: a 405 states which methods the target does serve.
        assert_eq!(
            refused.allow, "GET, HEAD",
            "case {method}: expected Allow: GET, HEAD on the refusal, got {:?}",
            refused.allow
        );
    }
}

/// `method uri`, unsigned and addressed to this service's endpoint: how a caller uses a
/// URL it was handed.
async fn request(router: &MultiServiceRouter, method: &str, uri: &str) -> Answer {
    let built = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
        .body(Bytes::new())
        .expect("the test request is well formed");
    dispatch(router, built).await
}

#[tokio::test]
async fn an_endpoint_carried_only_on_the_request_uri_keeps_its_port() {
    let router = emulator();
    let rows = catalog(&router).await;
    let (id, _) = rows.first().expect("the catalog publishes a report");

    // A request that carries no `Host` header at all names its endpoint on the URI's
    // AUTHORITY — which is where the PORT lives. The rule generalises over the endpoint,
    // so the rows are authorities differing in what they carry: a bare name, a name with
    // a port, and a loopback address with a port. Each must be announced whole, port
    // included, or the announced URL would not resolve back to the socket the caller
    // reached.
    for authority in [
        "artifact.eu-west-1.amazonaws.com",
        "artifact.eu-west-1.amazonaws.com:8443",
        "127.0.0.1:9297",
    ] {
        let term = send_without_host_header(
            &router,
            &format!("https://{authority}/v1/report/getTermForReport?reportId={id}"),
            true,
        )
        .await;
        assert_eq!(
            term.status, 200,
            "case {authority:?}: expected HTTP 200 from GetTermForReport, got {} with {:?}",
            term.status, term.body
        );
        let announced = term.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            announced.starts_with(&format!("https://{authority}/v1/report/download/")),
            "case {authority:?}: the announced URL must keep the whole authority the \
             request carried, got {announced:?}"
        );

        // And it resolves there: the same absolute-form fetch, still without a `Host`
        // header and unsigned, serves the term document.
        let document = send_without_host_header(&router, &announced, false).await;
        assert_eq!(
            document.status, 200,
            "case {authority:?}: the announced URL must serve its document, got {} with {:?}",
            document.status,
            String::from_utf8_lossy(&document.bytes)
        );
        assert!(
            !document.bytes.is_empty(),
            "case {authority:?}: the served term document must have content"
        );
    }
}
