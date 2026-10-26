// SPDX-License-Identifier: CC0-1.0

//! The export document plane behind the URL `ExportComplianceInquiry` announces.
//!
//! One rule is pinned here, over a SET of endpoints rather than any one of them: **a
//! document URL this emulator announces names the endpoint the request reached it on, and
//! a plain unsigned fetch of that URL serves the document.** The export plane is held to
//! the same rule the report and term plane already follows
//! (`document_data_plane.rs`), because a presigned URL is a promise that whoever was
//! handed it can fetch it: a URL naming a host this emulator does not serve is a dead
//! link.
//!
//! Every assertion goes through the router, so each case exercises the same path a client
//! takes: claiming, routing, the handler, the persisted state and serialization.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a control-plane request to
/// it.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The path prefix the export documents are published under (`crate::state`'s
/// `EXPORT_DOCUMENT_PATH_PREFIX`), spelled out here so a change to it has to be made
/// deliberately in both places.
const EXPORT_DOCUMENT_PATH_PREFIX: &str = "/v1/compliance-inquiry/download";

/// The host an announced URL names when the request carried no endpoint to read: a
/// loopback literal (`crate::state`'s `LOOPBACK_HOST`), because this emulator is what
/// serves the document.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// Long enough that the inquiry's processing span is over. The handlers store the due
/// instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature.
const PAST_THE_PROCESSING_SPAN: std::time::Duration = std::time::Duration::from_millis(900);

/// What the emulator answered: the status, the two headers a download carries, the raw
/// bytes and the JSON body when there is one.
struct Answer {
    status: u16,
    content_type: String,
    content_length: String,
    bytes: Bytes,
    body: Value,
}

/// One emulator over `engine`, with request authentication on or off.
fn emulator_over(engine: &Arc<engine::InMemoryEngine>, authenticated: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Send one request: `method uri`, addressed to `host` when the case names one (a case
/// that names none carries its endpoint on the URI's authority, as an HTTP/2 or
/// SDK-built request does), signed only when `signed`.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    host: Option<&str>,
    signed: bool,
    body: Bytes,
) -> Answer {
    let mut request = ::http::Request::builder().method(method).uri(uri);
    if let Some(host) = host {
        request = request.header("host", host);
    }
    if signed {
        request = request
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json");
    }
    let request = request
        .body(body)
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
    let bytes = response.into_body();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        content_type,
        content_length,
        bytes,
        body,
    }
}

/// Create one inquiry through the endpoint `host` names, and answer its id.
async fn create_inquiry(router: &MultiServiceRouter, uri_host: &UriHost, name: &str) -> String {
    let body = json!({
        "name": name,
        "inquiryContent": { "query": "Does the SOC 2 report cover this region?" },
    });
    let created = send(
        router,
        "POST",
        &uri_host.uri("/v1/compliance-inquiry/create"),
        uri_host.header(),
        true,
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {:?}: the create must succeed, got {} with {:?}",
        uri_host.label, created.status, created.body
    );
    created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the create carries no inquiry id: {:?}",
                uri_host.label, created.body
            )
        })
        .to_string()
}

/// Export one inquiry through the endpoint `uri_host` names, and answer the
/// `documentPresignedUrl` it announced.
async fn export_inquiry(
    router: &MultiServiceRouter,
    uri_host: &UriHost,
    inquiry_id: &str,
) -> String {
    let body = json!({ "complianceInquiryId": inquiry_id });
    let exported = send(
        router,
        "POST",
        &uri_host.uri("/v1/compliance-inquiry/export"),
        uri_host.header(),
        true,
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "case {:?}: ExportComplianceInquiry must answer HTTP 200, got {} with {:?}",
        uri_host.label, exported.status, exported.body
    );
    exported.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the export carries no documentPresignedUrl: {:?}",
                uri_host.label, exported.body
            )
        })
        .to_string()
}

/// How one case addresses the emulator: the endpoint it reached, and whether that
/// endpoint travels in a `Host` header or on the request URI's authority.
struct UriHost {
    label: &'static str,
    /// The authority the request carries, which the announced URL must name.
    authority: &'static str,
    /// True when the endpoint travels on the URI (absolute request form) instead of in a
    /// `Host` header.
    on_uri: bool,
}

impl UriHost {
    /// The request URI for `path_and_query`, in whichever form this case carries.
    fn uri(&self, path_and_query: &str) -> String {
        if self.on_uri {
            format!("https://{}{path_and_query}", self.authority)
        } else {
            path_and_query.to_string()
        }
    }

    /// The `Host` header this case sends, if any.
    fn header(&self) -> Option<&str> {
        (!self.on_uri).then_some(self.authority)
    }
}

/// The endpoints a caller can reach this emulator on. The rows differ along everything
/// the rule generalises over: a service endpoint host, that host with an explicit port, a
/// loopback address with a port, a loopback NAME with a port, and an endpoint carried only
/// on the request URI's authority rather than in a `Host` header.
fn endpoints() -> Vec<UriHost> {
    vec![
        UriHost {
            label: "this service's endpoint host",
            authority: "artifact.eu-west-1.amazonaws.com",
            on_uri: false,
        },
        UriHost {
            label: "an endpoint host with a port",
            authority: "artifact.eu-west-1.amazonaws.com:8443",
            on_uri: false,
        },
        UriHost {
            label: "the emulator's own socket",
            authority: "127.0.0.1:9297",
            on_uri: false,
        },
        UriHost {
            label: "a loopback name with a port",
            authority: "localhost:9297",
            on_uri: false,
        },
        UriHost {
            label: "an endpoint carried only on the request URI",
            authority: "artifact.eu-west-1.amazonaws.com:9297",
            on_uri: true,
        },
    ]
}

/// The path and query of an announced URL, asserting it names `authority` — which is what
/// makes it resolve for whoever was handed it.
fn path_of(url: &str, authority: &str, label: &str) -> String {
    let after_scheme = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {label:?}: an announced URL carries a scheme: {url:?}"));
    let (host, path) = after_scheme
        .split_once('/')
        .unwrap_or_else(|| panic!("case {label:?}: an announced URL carries a path: {url:?}"));
    assert_eq!(
        host, authority,
        "case {label:?}: the announced URL must name the endpoint the request reached, got \
         {url:?}"
    );
    format!("/{path}")
}

#[tokio::test]
async fn an_announced_export_url_names_the_caller_s_endpoint_and_serves_the_document() {
    for endpoint in endpoints() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_over(&engine, false);
        // A second front door over the SAME engine with request authentication ON: a
        // presigned URL is fetched with no `Authorization` header at all, so it has to be
        // served unsigned even where the control plane is not.
        let authenticated = emulator_over(&engine, true);
        let label = endpoint.label;

        let id = create_inquiry(&router, &endpoint, "Export URL resolves").await;
        std::thread::sleep(PAST_THE_PROCESSING_SPAN);
        let announced = export_inquiry(&router, &endpoint, &id).await;

        let expected_prefix =
            format!("https://{}{EXPORT_DOCUMENT_PATH_PREFIX}/{id}/export/", endpoint.authority);
        assert!(
            announced.starts_with(&expected_prefix),
            "case {label:?}: expected a URL under {expected_prefix:?}, got {announced:?}"
        );
        let path = path_of(&announced, endpoint.authority, label);

        // Followed exactly as a client follows a presigned URL: the method and path the
        // URL names, addressed to the endpoint it names, and with no credential at all.
        let fetched = send(
            &authenticated,
            "GET",
            &path,
            Some(endpoint.authority),
            false,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: the announced URL must serve the export document, got {} with {:?}",
            fetched.status,
            String::from_utf8_lossy(&fetched.bytes)
        );
        assert_eq!(
            fetched.content_type, "application/json",
            "case {label:?}: expected the document's own media type, got {:?}",
            fetched.content_type
        );
        assert_eq!(
            fetched.content_length,
            fetched.bytes.len().to_string(),
            "case {label:?}: content-length must be the document's length, got {:?} for {} \
             bytes",
            fetched.content_length,
            fetched.bytes.len()
        );
        assert_eq!(
            fetched.body["complianceInquiryId"].as_str(),
            Some(id.as_str()),
            "case {label:?}: the document must report the inquiry it was exported from, got \
             {:?}",
            fetched.body["complianceInquiryId"]
        );

        // The same fetch in ABSOLUTE request form, which is what an SDK-built request
        // carries, serves the same document.
        let absolute = send(
            &authenticated,
            "GET",
            &announced,
            Some(endpoint.authority),
            false,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            absolute.bytes, fetched.bytes,
            "case {label:?}: the announced URL must resolve in absolute request form too, got \
             {} with {:?}",
            absolute.status,
            String::from_utf8_lossy(&absolute.bytes)
        );

        // And however else the emulator is addressed: the PATH is what identifies a
        // document fetch, so a client that reaches the emulator on another endpoint, or
        // with no `Host` header at all, is served the same document.
        for (how, host, uri) in [
            ("addressed to another endpoint", Some("127.0.0.1:9297"), path.clone()),
            (
                "with no Host header at all",
                None,
                format!("https://127.0.0.1:9297{path}"),
            ),
        ] {
            let elsewhere = send(&authenticated, "GET", &uri, host, false, Bytes::new()).await;
            assert_eq!(
                elsewhere.bytes, fetched.bytes,
                "case {label:?} ({how}): a document URL must resolve however the emulator is \
                 addressed, got {} with {:?}",
                elsewhere.status,
                String::from_utf8_lossy(&elsewhere.bytes)
            );
        }
    }
}

#[tokio::test]
async fn a_host_that_is_not_an_endpoint_is_not_announced() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(&engine, false);

    // A `Host` the caller chose is only honoured when it IS an endpoint. The rows differ
    // along what makes a value something other than one: a path, a query, userinfo, a
    // space, and the empty value. Each must announce the loopback literal instead of
    // being reflected back into a URL this service hands out.
    for forged in [
        "elsewhere.example.com/v1/compliance-inquiry/download",
        "elsewhere.example.com?x=1",
        "user@elsewhere.example.com",
        "elsewhere example com",
        "",
    ] {
        let endpoint = UriHost {
            label: "a forged Host",
            authority: "artifact.eu-west-1.amazonaws.com",
            on_uri: false,
        };
        // An export reports on the inquiry whatever status the clock has brought it to,
        // so this case does not wait the processing span out.
        let id = create_inquiry(&router, &endpoint, "Forged host").await;

        let body = json!({ "complianceInquiryId": id });
        let exported = send(
            &router,
            "POST",
            "/v1/compliance-inquiry/export",
            Some(forged),
            true,
            Bytes::from(body.to_string()),
        )
        .await;
        assert_eq!(
            exported.status, 200,
            "case {forged:?}: the export must still succeed, got {} with {:?}",
            exported.status, exported.body
        );
        let announced = exported.body["documentPresignedUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_prefix = format!("https://{LOOPBACK_HOST}{EXPORT_DOCUMENT_PATH_PREFIX}/");
        assert!(
            announced.starts_with(&expected_prefix),
            "case {forged:?}: a Host that is not an endpoint must not be announced, expected a \
             URL under {expected_prefix:?}, got {announced:?}"
        );

        // And the loopback URL it announced instead resolves, which is what announcing it
        // is for.
        let path = path_of(&announced, LOOPBACK_HOST, forged);
        let fetched = send(&router, "GET", &path, Some(LOOPBACK_HOST), false, Bytes::new()).await;
        assert_eq!(
            fetched.status, 200,
            "case {forged:?}: the announced loopback URL must serve the document, got {} with \
             {:?}",
            fetched.status,
            String::from_utf8_lossy(&fetched.bytes)
        );
    }
}
