// SPDX-License-Identifier: CC0-1.0

//! The download URL `GetDownloadUrlForLayer` hands out, and the plane that serves it.
//!
//! One rule is pinned here, over a SET of layers, endpoints and windows rather than any
//! one of them: **for a layer the named repository holds, `GetDownloadUrlForLayer`
//! answers a PRE-SIGNED URL naming the endpoint the caller reached this emulator on, and
//! a fetch of that URL as it was announced — carrying no credential of the holder's own —
//! serves the registered blob's own bytes, byte for byte, while the grant behind it is
//! unexpired.** A layer the repository holds no blob under has no URL at all, a URL this
//! registry never handed out serves nothing, and one past its window is an expired
//! request rather than a missing one. What the URL's own signature covers, and what a
//! rewritten one is refused with, is `layer_download_signature.rs`' rule.
//!
//! Every case goes through the router, so each exercises the same path a client takes:
//! claiming, routing, the handler, the persisted state and serialization. The push itself
//! goes through the documented sequence (`crate::wire::given_layer`), so the bytes a
//! download serves are the bytes `UploadLayerPart` streamed in.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_engine, given_layer, layer_digest, ok, REGION, REGISTRY_ID};

/// The path space this registry publishes layer downloads under
/// (`state::LAYER_DOWNLOAD_PATH_PREFIX`), spelled out here so a change to it has to be
/// made deliberately in both places.
const DOWNLOAD_PATH_PREFIX: &str = "/ecr-layer-download";

/// The media type a layer download is served under, spelled out here for the same reason.
const DOWNLOAD_CONTENT_TYPE: &str = "application/octet-stream";

/// The host an announced URL names when the request carried no endpoint this emulator
/// could read.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The endpoint `crate::wire`'s calls are addressed to, which is therefore the authority
/// the URLs they are answered with must name.
const SERVICE_ENDPOINT: &str = "api.ecr.eu-west-1.amazonaws.com";

/// A SigV4 credential scope naming this service: what routes a control-plane call to ECR
/// whatever endpoint it is addressed to.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// What a fetch of a download URL answered: the status, the two headers a download
/// carries, the raw bytes, and the body as text — the `<Error>` document when the answer
/// is a refusal.
struct Fetched {
    status: u16,
    content_type: String,
    content_length: String,
    bytes: Bytes,
    body: String,
}

impl Fetched {
    /// The error code the `<Error>` document states, or the empty string on a served
    /// download.
    fn code(&self) -> &str {
        element(&self.body, "Code")
    }

    /// The message it states, or the empty string on a served download.
    fn message(&self) -> &str {
        element(&self.body, "Message")
    }
}

/// The text of `<name>…</name>` in `document`, or the empty string when it carries none —
/// enough of an XML reading for the three-element error document this plane answers with.
fn element<'d>(document: &'d str, name: &str) -> &'d str {
    document
        .split_once(&format!("<{name}>"))
        .and_then(|(_before, rest)| rest.split_once(&format!("</{name}>")))
        .map_or("", |(text, _after)| text)
}

/// One emulator over `engine`, with request authentication on or off.
fn emulator_over(engine: &Arc<engine::InMemoryEngine>, authenticated: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Follow a URL exactly as a client follows a pre-signed one: `method uri`, addressed to
/// the endpoint it names, and with NO credential of any kind.
async fn fetch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    host: Option<&str>,
) -> Fetched {
    let mut request = ::http::Request::builder().method(method).uri(uri);
    if let Some(host) = host {
        request = request.header("host", host);
    }
    let request = request
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
    let bytes = response.into_body();
    let body = String::from_utf8_lossy(&bytes).to_string();
    Fetched {
        status,
        content_type,
        content_length,
        bytes,
        body,
    }
}

/// One control-plane call on the awsJson1_1 wire, addressed to the endpoint `endpoint`
/// describes rather than to `crate::wire`'s fixed one.
async fn call_at(
    router: &MultiServiceRouter,
    endpoint: &Endpoint,
    operation: &str,
    body: &Value,
) -> Value {
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri(endpoint.uri("/"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AmazonEC2ContainerRegistry_V20150921.{operation}"),
        );
    if let Some(host) = endpoint.host_header() {
        request = request.header("host", host);
    }
    let request = request
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answered: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from {operation}, got {status} with {answered:?}",
        endpoint.label
    );
    answered
}

/// How one case addresses the emulator: what its `Host` header carries, what its request
/// URI carries, and the endpoint the URL it is answered with has to name.
struct Endpoint {
    label: &'static str,
    /// The `Host` header the request sends, or `None` for a request that sends none (as
    /// an HTTP/2 request does).
    host: Option<&'static str>,
    /// The authority the request URI carries in absolute form, or `None` for an
    /// origin-form request URI, which carries none.
    uri_authority: Option<&'static str>,
    /// The authority the URL this emulator announces must name: the first source the
    /// request carried that IS an endpoint, or the loopback literal when neither is.
    announced: &'static str,
}

impl Endpoint {
    /// The request URI for `path`, in whichever form this case carries.
    fn uri(&self, path: &str) -> String {
        match self.uri_authority {
            Some(authority) => format!("https://{authority}{path}"),
            None => path.to_string(),
        }
    }

    /// The `Host` header this case sends, if any.
    fn host_header(&self) -> Option<&str> {
        self.host
    }
}

/// The endpoints a caller can reach this emulator on. The rows differ along everything
/// the rule generalises over: which source carries the endpoint (a `Host` header, the
/// request URI's authority, both, neither), the forms a bare authority takes (a service
/// endpoint host, that host with an explicit port, a loopback socket, a loopback NAME with
/// a port), and three values that are not an endpoint at all — a host with a path, one
/// with userinfo, and one with a space — which must not be reflected into a URL this
/// registry hands out, each of those also crossed with a request URI that carries the
/// endpoint the caller really reached.
fn endpoints() -> Vec<Endpoint> {
    vec![
        Endpoint {
            label: "this service's endpoint host",
            host: Some(SERVICE_ENDPOINT),
            uri_authority: None,
            announced: SERVICE_ENDPOINT,
        },
        Endpoint {
            label: "an endpoint host with a port",
            host: Some("api.ecr.eu-west-1.amazonaws.com:8443"),
            uri_authority: None,
            announced: "api.ecr.eu-west-1.amazonaws.com:8443",
        },
        Endpoint {
            label: "the emulator's own socket",
            host: Some("127.0.0.1:9297"),
            uri_authority: None,
            announced: "127.0.0.1:9297",
        },
        Endpoint {
            label: "a loopback name with a port",
            host: Some("localhost:9297"),
            uri_authority: None,
            announced: "localhost:9297",
        },
        Endpoint {
            label: "an endpoint carried only on the request URI",
            host: None,
            uri_authority: Some("api.ecr.eu-west-1.amazonaws.com:9297"),
            announced: "api.ecr.eu-west-1.amazonaws.com:9297",
        },
        Endpoint {
            label: "both sources usable: the Host header is the one the caller addressed",
            host: Some("localhost:9297"),
            uri_authority: Some("127.0.0.1:443"),
            announced: "localhost:9297",
        },
        Endpoint {
            label: "a host carrying a path",
            host: Some("elsewhere.example.com/ecr-layer-download"),
            uri_authority: None,
            announced: LOOPBACK_HOST,
        },
        Endpoint {
            label: "a host carrying userinfo",
            host: Some("user@elsewhere.example.com"),
            uri_authority: None,
            announced: LOOPBACK_HOST,
        },
        Endpoint {
            label: "a host carrying a space",
            host: Some("elsewhere example com"),
            uri_authority: None,
            announced: LOOPBACK_HOST,
        },
        Endpoint {
            label: "a host carrying a path, the endpoint on the request URI",
            host: Some("elsewhere.example.com/ecr-layer-download"),
            uri_authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Endpoint {
            label: "a host carrying userinfo, the endpoint on the request URI",
            host: Some("user@elsewhere.example.com"),
            uri_authority: Some("localhost:9297"),
            announced: "localhost:9297",
        },
        Endpoint {
            label: "a host carrying a space, the endpoint on the request URI",
            host: Some("elsewhere example com"),
            uri_authority: Some("api.ecr.eu-west-1.amazonaws.com:9297"),
            announced: "api.ecr.eu-west-1.amazonaws.com:9297",
        },
        Endpoint {
            label: "neither source carries an endpoint",
            host: None,
            uri_authority: None,
            announced: LOOPBACK_HOST,
        },
    ]
}

/// `len` layer bytes that depend on `seed`: a deterministic spread over the whole byte
/// range, so a layer carries `0x00` and `0xff` like real compressed layer content and two
/// layers do not carry the same bytes.
fn layer_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 37 + seed * 101 + 7) % 256).unwrap_or_default())
        .collect()
}

/// The path and query of an announced URL, asserting it names `authority` — which is what
/// makes it resolve for whoever was handed it.
fn path_and_query(url: &str, authority: &str, case: &str) -> String {
    let after_scheme = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case:?}: an announced URL carries a scheme: {url:?}"));
    let (host, rest) = after_scheme
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: an announced URL carries a path: {url:?}"));
    assert_eq!(
        host, authority,
        "case {case:?}: the announced URL must name the endpoint the request reached, got \
         {url:?}"
    );
    format!("/{rest}")
}

/// The grant token an announced URL carries: the segment after the published prefix.
fn token_of(url: &str, case: &str) -> String {
    let path = url.split('?').next().unwrap_or_default();
    let rest = path
        .split_once(&format!("{DOWNLOAD_PATH_PREFIX}/"))
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| {
            panic!("case {case:?}: an announced URL is published under the download prefix: {url:?}")
        });
    rest.split('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `target` (a path under the published download prefix) as this registry would announce
/// it: signed through the service's OWN signing path, at this instant, for the window it
/// mints with. The signature is then one this registry computed, so a case built on it
/// exercises what the grant resolves to rather than whether the address was signed.
fn signed(target: &str) -> String {
    state::presigned_layer_download_url(
        SERVICE_ENDPOINT,
        target,
        REGION,
        SystemTime::now(),
        state::LAYER_DOWNLOAD_URL_TTL,
    )
}

/// Ask for the download URL of `digest` in `repository` through `crate::wire`'s endpoint,
/// and answer the whole response.
async fn download_url(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Value {
    ok(
        router,
        case,
        "GetDownloadUrlForLayer",
        &json!({"repositoryName": repository, "layerDigest": digest}),
    )
    .await
}

/// Create `repository` and push `content` into it as a layer, answering the digest the
/// registry registered it under.
async fn given_repository_layer(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    content: &[u8],
) -> String {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    given_layer(router, case, repository, content).await
}

/// A download URL serves the layer's own bytes, byte for byte, to a caller holding no
/// credential at all — and serves them as often as it is followed.
///
/// The rows differ along what the rule generalises over: the size of the layer, the byte
/// values in it, and whether its bytes repeat — a layer is opaque content, so none of
/// that is part of the rule. The bytes asserted are the ones pushed, and the digest of
/// what came back is compared against the digest the registry answered with, so a plane
/// that served a truncated or re-encoded body fails.
#[tokio::test]
async fn a_download_url_serves_the_exact_layer_bytes() {
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("a single-byte layer", "onebyte", vec![0x5a]),
        ("a short text layer", "shorttext", b"a layer of text".to_vec()),
        (
            "a layer carrying every byte value",
            "everybyte",
            (0..=255_u8).collect(),
        ),
        ("a layer whose bytes repeat", "repeated", vec![0xff; 4_096]),
        ("a layer of tens of kilobytes", "tensofkb", layer_bytes(3, 65_536)),
        ("a layer of an odd length", "oddlength", layer_bytes(5, 4_097)),
    ];

    let (router, engine) = emulator_with_engine();
    // A second front door over the SAME engine with request authentication ON: a
    // pre-signed URL is fetched with no `Authorization` header at all, so it has to be
    // served unsigned even where the control plane is not.
    let authenticated = emulator_over(&engine, true);

    for (case, repository, content) in cases {
        let digest = given_repository_layer(&router, case, repository, &content).await;
        let answer = download_url(&router, case, repository, &digest).await;

        assert_eq!(
            answer["layerDigest"].as_str(),
            Some(digest.as_str()),
            "case {case:?}: the answer names the layer asked for, got {:?}",
            answer["layerDigest"]
        );
        let url = answer["downloadUrl"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the answer carries no downloadUrl: {answer:?}")
            })
            .to_string();
        let prefix = format!("https://{SERVICE_ENDPOINT}{DOWNLOAD_PATH_PREFIX}/");
        assert!(
            url.starts_with(&prefix),
            "case {case:?}: expected a URL under {prefix:?}, got {url:?}"
        );
        assert!(
            url.contains(&digest),
            "case {case:?}: the URL names the layer it leads to, got {url:?}"
        );
        assert!(
            url.contains(&format!(
                "X-Amz-Expires={}",
                state::LAYER_DOWNLOAD_URL_TTL.as_secs()
            )),
            "case {case:?}: the URL states the window it was minted with, got {url:?}"
        );

        let path = path_and_query(&url, SERVICE_ENDPOINT, case);
        let fetched = fetch(&authenticated, "GET", &path, Some(SERVICE_ENDPOINT)).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the announced URL must serve the layer, got {} with {:?}",
            fetched.status,
            String::from_utf8_lossy(&fetched.bytes)
        );
        assert_eq!(
            fetched.bytes.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the {} pushed bytes, got {}",
            content.len(),
            fetched.bytes.len()
        );
        assert_eq!(
            layer_digest(&fetched.bytes),
            digest,
            "case {case:?}: the bytes served must be the content the digest addresses"
        );
        assert_eq!(
            fetched.content_type, DOWNLOAD_CONTENT_TYPE,
            "case {case:?}: expected the generic binary media type, got {:?}",
            fetched.content_type
        );
        assert_eq!(
            fetched.content_length,
            content.len().to_string(),
            "case {case:?}: content-length must be the layer's length, got {:?} for {} \
             bytes",
            fetched.content_length,
            content.len()
        );

        // A `HEAD` reports the layer's length without its bytes.
        let head = fetch(&authenticated, "HEAD", &path, Some(SERVICE_ENDPOINT)).await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: a HEAD of the URL must be served, got {} with {:?}",
            head.status, head.body
        );
        assert!(
            head.bytes.is_empty(),
            "case {case:?}: a HEAD carries no body, got {} bytes",
            head.bytes.len()
        );
        assert_eq!(
            head.content_length,
            content.len().to_string(),
            "case {case:?}: a HEAD still reports the layer's length, got {:?}",
            head.content_length
        );

        // A download is a pure read: following the same URL again serves the same layer.
        let again = fetch(&authenticated, "GET", &path, Some(SERVICE_ENDPOINT)).await;
        assert_eq!(
            again.bytes, fetched.bytes,
            "case {case:?}: following a URL twice must serve the same layer, got {} with \
             {} bytes",
            again.status,
            again.bytes.len()
        );

        // The same fetch in ABSOLUTE request form, which is what an SDK-built request
        // carries: the path and query are what identify a download, and they are the same
        // two halves the signature covers either way.
        let absolute = fetch(&authenticated, "GET", &url, Some(SERVICE_ENDPOINT)).await;
        assert_eq!(
            absolute.bytes, fetched.bytes,
            "case {case:?}: the URL must resolve in absolute request form too, got {}",
            absolute.status
        );
        // The endpoint the URL names is one of the headers its signature covers, so
        // following it through ANOTHER endpoint presents a signature that was not computed
        // over what was presented — which the object store a pre-signed download is served
        // from refuses rather than serving the layer.
        let elsewhere = fetch(&authenticated, "GET", &path, Some("127.0.0.1:9297")).await;
        assert_eq!(
            elsewhere.status, 403,
            "case {case:?}: a URL signed over one endpoint must not resolve on another, \
             got {} with {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_eq!(
            elsewhere.code(),
            "SignatureDoesNotMatch",
            "case {case:?}: expected SignatureDoesNotMatch, got {:?}",
            elsewhere.body
        );
        assert_ne!(
            elsewhere.bytes.as_ref(),
            content.as_slice(),
            "case {case:?}: a refused fetch serves no layer byte"
        );

        // Each call mints its own capability, as each pre-signed URL AWS answers with is
        // its own, and both of them resolve to the one layer.
        let second = download_url(&router, case, repository, &digest).await;
        let second_url = second["downloadUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            second_url, url,
            "case {case:?}: a second call must hand out its own URL, got {second_url:?}"
        );
        let from_second = fetch(
            &authenticated,
            "GET",
            &path_and_query(&second_url, SERVICE_ENDPOINT, case),
            Some(SERVICE_ENDPOINT),
        )
        .await;
        assert_eq!(
            from_second.bytes, fetched.bytes,
            "case {case:?}: the second URL must serve the same layer, got {} with {} \
             bytes",
            from_second.status,
            from_second.bytes.len()
        );
    }
}

/// The URL names the endpoint the CALLER reached this emulator on, and that URL resolves.
///
/// The rows are [`endpoints`]: a service host, a host with a port, a loopback socket, a
/// loopback name, an endpoint carried only on the request URI, a request carrying both,
/// three values that are not an endpoint at all — which must be answered with the loopback
/// literal rather than reflected into a URL this registry hands out — and those same three
/// beside an endpoint on the request URI, which is then what the URL names, because the
/// sources are read in order and the FIRST that is an endpoint wins.
#[tokio::test]
async fn a_download_url_names_the_endpoint_the_caller_reached() {
    let content = layer_bytes(7, 2_048);

    for endpoint in endpoints() {
        let (router, engine) = emulator_with_engine();
        let authenticated = emulator_over(&engine, true);
        let case = endpoint.label;
        let repository = "endpoints";

        call_at(
            &router,
            &endpoint,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        // The push itself goes through `crate::wire`'s endpoint; what this case varies is
        // the endpoint the URL is ASKED FOR on.
        let digest = given_layer(&router, case, repository, &content).await;

        let answer = call_at(
            &router,
            &endpoint,
            "GetDownloadUrlForLayer",
            &json!({"repositoryName": repository, "layerDigest": digest}),
        )
        .await;
        let url = answer["downloadUrl"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the answer carries no downloadUrl: {answer:?}")
            })
            .to_string();
        let expected = format!("https://{}{DOWNLOAD_PATH_PREFIX}/", endpoint.announced);
        assert!(
            url.starts_with(&expected),
            "case {case:?}: expected a URL under {expected:?}, got {url:?}"
        );

        let path = path_and_query(&url, endpoint.announced, case);
        let fetched = fetch(&authenticated, "GET", &path, Some(endpoint.announced)).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the announced URL must resolve, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.bytes.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the pushed layer, got {} bytes",
            fetched.bytes.len()
        );
    }
}

/// A layer the named repository holds no blob under has no download URL, and a refused
/// call hands out no capability at all.
///
/// The rows differ along the ways a request can fail to name a layer of this registry: a
/// repository that was never created, one that was deleted, a well-formed digest nothing
/// uploaded, a digest another repository holds (a blob is per-repository), and another
/// account's registry.
#[tokio::test]
async fn a_layer_the_repository_does_not_hold_has_no_download_url() {
    let (router, engine) = emulator_with_engine();
    let mine = layer_bytes(11, 512);
    let elsewhere = layer_bytes(13, 512);

    let held = given_repository_layer(&router, "layer scoping", "mine", &mine).await;
    let other = given_repository_layer(&router, "layer scoping", "other", &elsewhere).await;
    given_repository_layer(&router, "layer scoping", "gonerepo", &mine).await;
    ok(
        &router,
        "layer scoping",
        "DeleteRepository",
        &json!({"repositoryName": "gonerepo"}),
    )
    .await;

    // A well-formed `LayerDigest` this registry holds no blob under.
    let absent = format!("sha256:{}", "0".repeat(64));
    let cases: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a repository that was never created",
            json!({"repositoryName": "absent", "layerDigest": held}),
            "RepositoryNotFoundException",
            None,
        ),
        (
            "a repository that was deleted after the layer was pushed",
            json!({"repositoryName": "gonerepo", "layerDigest": held}),
            "RepositoryNotFoundException",
            None,
        ),
        (
            "a digest nothing uploaded",
            json!({"repositoryName": "mine", "layerDigest": absent}),
            "LayersNotFoundException",
            Some(state::layer_not_found_message(&absent, "mine")),
        ),
        (
            "a digest another repository holds",
            json!({"repositoryName": "mine", "layerDigest": other}),
            "LayersNotFoundException",
            Some(state::layer_not_found_message(&other, "mine")),
        ),
        (
            "another account's registry",
            json!({
                "registryId": "123456789012",
                "repositoryName": "mine",
                "layerDigest": held,
            }),
            "InvalidParameterException",
            None,
        ),
    ];

    for (case, request, code, message) in cases {
        let answer = call(&router, "GetDownloadUrlForLayer", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
        if let Some(message) = message {
            assert_eq!(
                answer.message(),
                message,
                "case {case:?}: expected the refusal to name the digest, the repository \
                 and the registry, got {:?}",
                answer.body
            );
        }
        assert!(
            answer.body["downloadUrl"].is_null(),
            "case {case:?}: a refusal hands out no URL, got {:?}",
            answer.body["downloadUrl"]
        );
        let grants = state::service_state(&*engine)
            .expect("the ecr store is reachable from the engine the router was built over")
            .lock()
            .layer_download_grants
            .len();
        assert_eq!(
            grants, 0,
            "case {case:?}: a refused call stores no download grant, got {grants}"
        );
    }

    // The layer the refusals above were refused for is served when it is asked for in its
    // own repository, so each refusal is about what was named and not about the layer
    // being unreachable.
    let answer = download_url(&router, "the layer's own repository", "mine", &held).await;
    assert_eq!(
        answer["layerDigest"].as_str(),
        Some(held.as_str()),
        "the layer of the repository that holds it has a URL, got {answer:?}"
    );
    assert_eq!(
        answer["registryId"], Value::Null,
        "GetDownloadUrlForLayer's modelled output carries no registryId, got {answer:?}"
    );
    assert_eq!(
        state::registry_id(),
        REGISTRY_ID,
        "this emulator's registry id is the account it vends; {REGION} changes nothing \
         about it"
    );
}

/// A download URL serves the layer while its window is open and answers an EXPIRED
/// request once it has closed; the grant it is addressed through is dropped when the next
/// call mints one, after which the same URL names nothing.
///
/// The rows differ along where the grant's window sits relative to the clock: still open,
/// one second closed, a minute closed and a day closed — the bound the rule turns on. The
/// window is moved in the store, because no reasonable number of wire calls waits an hour
/// out; what the fetch is judged against is the stored grant either way.
#[tokio::test]
async fn a_download_url_stops_serving_when_its_window_closes() {
    let (router, engine) = emulator_with_engine();
    let authenticated = emulator_over(&engine, true);
    let content = layer_bytes(17, 1_024);
    let repository = "expiring";
    let digest = given_repository_layer(&router, "the download window", repository, &content).await;

    let cases: Vec<(&str, Option<Duration>)> = vec![
        ("a window still open", None),
        ("a window one second closed", Some(Duration::from_secs(1))),
        ("a window a minute closed", Some(Duration::from_secs(60))),
        (
            "a window a day closed",
            Some(Duration::from_secs(24 * 60 * 60)),
        ),
    ];

    for (case, closed_for) in cases {
        let answer = download_url(&router, case, repository, &digest).await;
        let url = answer["downloadUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let token = token_of(&url, case);
        let path = path_and_query(&url, SERVICE_ENDPOINT, case);

        // The window the registry minted it with is the one the URL states.
        {
            let store = state::service_state(&*engine).expect("the ecr store is reachable");
            let mut store = store.lock();
            let grant = store
                .layer_download_grants
                .get_mut(&token)
                .unwrap_or_else(|| panic!("case {case:?}: the grant just minted is in the store"));
            assert_eq!(
                grant.validity_seconds(),
                state::LAYER_DOWNLOAD_URL_TTL.as_secs(),
                "case {case:?}: a fresh grant is valid for the registry's window, got {} \
                 seconds",
                grant.validity_seconds()
            );
            assert_eq!(
                grant.layer_digest, digest,
                "case {case:?}: the grant names the layer asked for, got {:?}",
                grant.layer_digest
            );
            assert_eq!(
                grant.repository_name, repository,
                "case {case:?}: the grant names the repository asked for, got {:?}",
                grant.repository_name
            );
            // The window is moved so the case can observe the bound without waiting.
            if let Some(closed_for) = closed_for {
                let closed_at = SystemTime::now() - closed_for;
                grant.expires_at = closed_at;
                grant.issued_at = closed_at - state::LAYER_DOWNLOAD_URL_TTL;
                assert!(
                    grant.has_expired(SystemTime::now()),
                    "case {case:?}: the moved window must be closed"
                );
            }
        }

        let fetched = fetch(&authenticated, "GET", &path, Some(SERVICE_ENDPOINT)).await;
        match closed_for {
            None => {
                assert_eq!(
                    fetched.status, 200,
                    "case {case:?}: an open window serves the layer, got {} with {:?}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.bytes.as_ref(),
                    content.as_slice(),
                    "case {case:?}: expected the pushed layer, got {} bytes",
                    fetched.bytes.len()
                );
            }
            Some(_) => {
                assert_eq!(
                    fetched.status, 403,
                    "case {case:?}: a closed window refuses the fetch, got {} with {:?}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    fetched.code(),
                    "AccessDenied",
                    "case {case:?}: expected AccessDenied, got {:?}",
                    fetched.body
                );
                assert_eq!(
                    fetched.message(),
                    "Request has expired",
                    "case {case:?}: expected the expiry wording, got {:?}",
                    fetched.body
                );
                assert_ne!(
                    fetched.bytes.as_ref(),
                    content.as_slice(),
                    "case {case:?}: a refused fetch serves no layer byte"
                );

                // A fresh URL for the same layer serves it, so the refusal is the window
                // closing and nothing else — and minting it reclaims the closed grant,
                // after which the old URL names no download rather than an expired one.
                let fresh = download_url(&router, case, repository, &digest).await;
                let fresh_url = fresh["downloadUrl"].as_str().unwrap_or_default().to_string();
                let served = fetch(
                    &authenticated,
                    "GET",
                    &path_and_query(&fresh_url, SERVICE_ENDPOINT, case),
                    Some(SERVICE_ENDPOINT),
                )
                .await;
                assert_eq!(
                    served.bytes.as_ref(),
                    content.as_slice(),
                    "case {case:?}: a freshly minted URL serves the layer, got {} with {} \
                     bytes",
                    served.status,
                    served.bytes.len()
                );
                let reclaimed = state::service_state(&*engine)
                    .expect("the ecr store is reachable")
                    .lock()
                    .layer_download_grant(&token)
                    .is_none();
                assert!(
                    reclaimed,
                    "case {case:?}: a closed grant is reclaimed when the next one is minted"
                );
                let after = fetch(&authenticated, "GET", &path, Some(SERVICE_ENDPOINT)).await;
                assert_eq!(
                    after.status, 404,
                    "case {case:?}: a reclaimed grant's URL names no download, got {} with \
                     {:?}",
                    after.status, after.body
                );
            }
        }
    }
}

/// Only a URL this registry handed out serves a layer: the capability is the grant its
/// token addresses, never the repository and digest spelled in the path.
///
/// The rows differ along the ways a path can fail to name a grant: a token this registry
/// never minted, a token that is not an id at all, a real token under another layer's
/// digest, and a real token whose repository has since been deleted. Each address is
/// SIGNED by this registry's own signing path, so what every row exercises is the grant
/// the token resolves to and not whether the address was signed (which
/// `layer_download_signature.rs` holds to its own rule). A method the plane does not serve
/// is refused as a method, naming the ones it does.
#[tokio::test]
async fn only_a_url_this_registry_handed_out_serves_a_layer() {
    let (router, engine) = emulator_with_engine();
    let authenticated = emulator_over(&engine, true);
    let mine = layer_bytes(19, 777);
    let other = layer_bytes(23, 333);

    let held = given_repository_layer(&router, "forged paths", "forged", &mine).await;
    let other_digest = given_repository_layer(&router, "forged paths", "another", &other).await;
    let gone_digest = given_repository_layer(&router, "forged paths", "deletedrepo", &mine).await;

    let answer = download_url(&router, "forged paths", "forged", &held).await;
    let url = answer["downloadUrl"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let token = token_of(&url, "forged paths");
    let path = path_and_query(&url, SERVICE_ENDPOINT, "forged paths");

    // A grant for a layer of a repository that is then deleted: the grant stands, and the
    // download reads the repository at the moment of the fetch.
    let gone = download_url(&router, "forged paths", "deletedrepo", &gone_digest).await;
    let gone_url = gone["downloadUrl"].as_str().unwrap_or_default().to_string();
    let gone_path = path_and_query(&gone_url, SERVICE_ENDPOINT, "a deleted repository");
    ok(
        &router,
        "forged paths",
        "DeleteRepository",
        &json!({"repositoryName": "deletedrepo"}),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "a token this registry never handed out",
            signed(&format!(
                "{DOWNLOAD_PATH_PREFIX}/{}/{held}",
                "a".repeat(64)
            )),
        ),
        (
            "a token that is not an id at all",
            signed(&format!("{DOWNLOAD_PATH_PREFIX}/not-a-token/{held}")),
        ),
        (
            "a real token under another layer's digest",
            signed(&format!("{DOWNLOAD_PATH_PREFIX}/{token}/{other_digest}")),
        ),
        (
            "a real token whose repository was deleted",
            gone_path.clone(),
        ),
    ];

    for (case, path) in cases {
        let fetched = fetch(&authenticated, "GET", &path, Some(SERVICE_ENDPOINT)).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.code(),
            "NoSuchKey",
            "case {case:?}: expected NoSuchKey, got {:?}",
            fetched.body
        );
        assert_ne!(
            fetched.bytes.as_ref(),
            mine.as_slice(),
            "case {case:?}: a path that names no grant serves no layer"
        );
        assert_ne!(
            fetched.bytes.as_ref(),
            other.as_slice(),
            "case {case:?}: a path that names no grant serves no other layer either"
        );
    }

    // A method the download plane does not serve is refused as a method, naming the two
    // it does — not answered as a missing download.
    let posted = {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(&path)
            .header("host", SERVICE_ENDPOINT)
            .body(Bytes::new())
            .expect("the test request is well formed");
        let response = authenticated.dispatch(request).await;
        let status = response.status().as_u16();
        let allow = response
            .headers()
            .get("allow")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        (status, allow)
    };
    assert_eq!(
        posted.0, 405,
        "a POST to a published download URL is refused as a method, got {}",
        posted.0
    );
    assert_eq!(
        posted.1, "GET, HEAD",
        "the refusal states the methods the download plane serves, got {:?}",
        posted.1
    );

    // And the URL the forged ones were modelled on still serves its layer.
    let served = fetch(&authenticated, "GET", &path, Some(SERVICE_ENDPOINT)).await;
    assert_eq!(
        served.bytes.as_ref(),
        mine.as_slice(),
        "the URL this registry handed out serves its layer, got {} with {} bytes",
        served.status,
        served.bytes.len()
    );
}
