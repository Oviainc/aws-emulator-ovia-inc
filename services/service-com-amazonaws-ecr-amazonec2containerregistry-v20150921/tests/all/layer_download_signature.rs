// SPDX-License-Identifier: CC0-1.0

//! The SIGNATURE on the download URL `GetDownloadUrlForLayer` hands out.
//!
//! Two rules are pinned here, each over a SET of inputs rather than any one of them:
//!
//! 1. every URL this registry announces is a pre-signed download address: it carries the
//!    six query-string fields SigV4 signs a download with — `X-Amz-Algorithm`,
//!    `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`, `X-Amz-SignedHeaders` and
//!    `X-Amz-Signature` — with the algorithm, scope, window and signed-header list the
//!    signature was computed under;
//! 2. the plane honours exactly what it signed: the address as announced serves the layer,
//!    while an address whose signing fields, query or path were rewritten — the stated
//!    window above all, which is one of the SIGNED fields and therefore not a client's to
//!    extend or shorten — is refused `403 SignatureDoesNotMatch`, an address presenting no
//!    signature at all is refused `403 AccessDenied`, and one whose signed window has
//!    closed is an expired request.
//!
//! Past the signature, which grant the token resolves to is `layer_download_url.rs`' rule;
//! these suites share the push sequence (`crate::wire::given_layer`) so the bytes a
//! download serves are the bytes `UploadLayerPart` streamed in.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::json;
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator_with_engine, given_layer, ok, ok_in_region, REGION};

/// The path space this registry publishes layer downloads under
/// (`state::LAYER_DOWNLOAD_PATH_PREFIX`), spelled out here so a change to it has to be
/// made deliberately in both places.
const DOWNLOAD_PATH_PREFIX: &str = "/ecr-layer-download";

/// The endpoint `crate::wire`'s calls are addressed to, and therefore the endpoint the
/// URLs they are answered with are signed over.
const SERVICE_ENDPOINT: &str = "api.ecr.eu-west-1.amazonaws.com";

/// The algorithm a SigV4 query-string signature names
/// (`X-Amz-Algorithm`, from AWS's "Authenticating Requests: Using Query Parameters").
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The six query fields a pre-signed download address carries, in the order AWS documents
/// them. Source: AWS's SigV4 query-string authentication reference.
const SIGNING_FIELDS: [&str; 6] = [
    "X-Amz-Algorithm",
    "X-Amz-Credential",
    "X-Amz-Date",
    "X-Amz-Expires",
    "X-Amz-SignedHeaders",
    "X-Amz-Signature",
];

/// What a fetch of a download URL answered: the status, the raw bytes and the body as
/// text — the `<Error>` document when the answer is a refusal.
struct Fetched {
    status: u16,
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

/// The text of `<name>…</name>` in `document`, or the empty string when it carries none.
fn element<'d>(document: &'d str, name: &str) -> &'d str {
    document
        .split_once(&format!("<{name}>"))
        .and_then(|(_before, rest)| rest.split_once(&format!("</{name}>")))
        .map_or("", |(text, _after)| text)
}

/// One emulator over `engine` with request authentication ON: a pre-signed URL is followed
/// with no `Authorization` header at all, so it has to be served where the control plane
/// is not.
fn authenticated_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Follow `uri` exactly as a client follows a pre-signed URL: no credential of any kind,
/// and the `Host` the address names.
async fn fetch(router: &MultiServiceRouter, method: &str, uri: &str, host: &str) -> Fetched {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = String::from_utf8_lossy(&bytes).to_string();
    Fetched {
        status,
        bytes,
        body,
    }
}

/// `len` layer bytes that depend on `seed`: a deterministic spread over the whole byte
/// range, so two layers do not carry the same bytes.
fn layer_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + seed * 97 + 13) % 256).unwrap_or_default())
        .collect()
}

/// Create `repository`, push `content` into it and ask for the download URL of the layer,
/// answering that URL.
async fn announced_url(
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
    let digest = given_layer(router, case, repository, content).await;
    let answer = ok(
        router,
        case,
        "GetDownloadUrlForLayer",
        &json!({"repositoryName": repository, "layerDigest": digest}),
    )
    .await;
    answer["downloadUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no downloadUrl: {answer:?}"))
        .to_string()
}

/// `url` split into the endpoint it names, the request target a signature covers (its path
/// and query) and that query on its own.
fn parts_of(url: &str) -> (String, String, String) {
    let after_scheme = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("an announced URL carries a scheme: {url:?}"));
    let (host, rest) = after_scheme
        .split_once('/')
        .unwrap_or_else(|| panic!("an announced URL carries a path: {url:?}"));
    let (_path, query) = rest
        .split_once('?')
        .unwrap_or_else(|| panic!("an announced URL carries a query string: {url:?}"));
    (host.to_string(), format!("/{rest}"), query.to_string())
}

/// `url` split into the request target a signature covers (its path and query) and that
/// query on its own.
fn target_and_query(url: &str) -> (String, String) {
    let (_host, target, query) = parts_of(url);
    (target, query)
}

/// The value of query field `name` in `url`, exactly as it was written.
fn field(url: &str, name: &str) -> String {
    let (_target, query) = target_and_query(url);
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(field, value)| (field == name).then(|| value.to_string()))
        .unwrap_or_default()
}

/// `url` with query field `name` set to `value`, every other pair left byte-identical —
/// which is what a client that rewrites one field of a pre-signed URL presents.
fn with_field(url: &str, name: &str, value: &str) -> String {
    let (target, query) = target_and_query(url);
    let rewritten: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((field, _)) if field == name => format!("{field}={value}"),
            _ => pair.to_string(),
        })
        .collect();
    let path = target.split('?').next().unwrap_or_default();
    format!("{path}?{}", rewritten.join("&"))
}

/// `url` with query field `name` removed altogether.
fn without_field(url: &str, name: &str) -> String {
    let (target, query) = target_and_query(url);
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| pair.split_once('=').map(|(field, _)| field) != Some(name))
        .collect();
    let path = target.split('?').next().unwrap_or_default();
    format!("{path}?{}", kept.join("&"))
}

/// RULE 1: every URL this registry announces carries the signature that authorises a
/// `GET` of its own address, under the algorithm, scope, window and signed-header list
/// that signature was computed with.
///
/// The rows differ along what the rule generalises over: the Region the call was made
/// against (which is the Region the credential scope names) and the layer the URL leads
/// to. Nothing asserted here is a value of one run: the scope's date is read back from the
/// URL's own `X-Amz-Date`, and the signature is held to its shape and to the fact that it
/// verifies, which the fetch below proves.
#[tokio::test]
async fn every_announced_download_url_is_presigned() {
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("eu-west-1", "regioneuwest", layer_bytes(1, 512)),
        ("us-east-1", "regionuseast", layer_bytes(2, 1_024)),
        ("ap-southeast-2", "regionapsoutheast", layer_bytes(3, 7)),
    ];

    for (region, repository, content) in cases {
        let (router, engine) = emulator_with_engine();
        let authenticated = authenticated_over(&engine);
        let case = region;

        ok_in_region(
            &router,
            case,
            region,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let digest = given_layer(&router, case, repository, &content).await;
        let answer = ok_in_region(
            &router,
            case,
            region,
            "GetDownloadUrlForLayer",
            &json!({"repositoryName": repository, "layerDigest": digest}),
        )
        .await;
        let url = answer["downloadUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the answer carries no downloadUrl"))
            .to_string();

        for name in SIGNING_FIELDS {
            assert!(
                !field(&url, name).is_empty(),
                "case {case:?}: a pre-signed download address carries {name}, got {url:?}"
            );
        }
        assert_eq!(
            field(&url, "X-Amz-Algorithm"),
            SIGV4_ALGORITHM,
            "case {case:?}: expected the SigV4 algorithm, got {url:?}"
        );
        // The credential names the signing key and the scope the signature is bound to:
        // the date the URL itself states, the Region the call was made against, this
        // service's signing name and the SigV4 terminator.
        let credential = field(&url, "X-Amz-Credential").replace("%2F", "/");
        let scope: Vec<&str> = credential.split('/').collect();
        assert_eq!(
            scope.len(),
            5,
            "case {case:?}: expected a five-part credential scope, got {credential:?}"
        );
        let date = field(&url, "X-Amz-Date");
        assert_eq!(
            scope.get(1).copied().unwrap_or_default(),
            date.get(..8).unwrap_or_default(),
            "case {case:?}: the scope's date is the URL's own signing date, got \
             {credential:?} for {date:?}"
        );
        assert_eq!(
            scope.get(2).copied().unwrap_or_default(),
            region,
            "case {case:?}: the scope names the Region the call was made against, got \
             {credential:?}"
        );
        assert_eq!(
            scope.get(3).copied().unwrap_or_default(),
            "ecr",
            "case {case:?}: the scope names the service that serves the download, got \
             {credential:?}"
        );
        assert_eq!(
            scope.get(4).copied().unwrap_or_default(),
            "aws4_request",
            "case {case:?}: the scope terminates in the SigV4 terminator, got \
             {credential:?}"
        );
        let signing_key = scope.first().copied().unwrap_or_default();
        assert_eq!(
            signing_key.len(),
            20,
            "case {case:?}: the credential names a 20-character signing key, got \
             {signing_key:?}"
        );
        assert_eq!(
            field(&url, "X-Amz-Expires"),
            state::LAYER_DOWNLOAD_URL_TTL.as_secs().to_string(),
            "case {case:?}: the URL states the window it was signed for, got {url:?}"
        );
        assert_eq!(
            field(&url, "X-Amz-SignedHeaders"),
            "host",
            "case {case:?}: the signature covers the host the URL is addressed to, got \
             {url:?}"
        );
        let signature = field(&url, "X-Amz-Signature");
        assert!(
            signature.len() == 64 && signature.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {case:?}: the URL carries a SHA-256 signature, got {signature:?}"
        );
        assert_eq!(
            date.len(),
            16,
            "case {case:?}: the signing date is the basic SigV4 form, got {date:?}"
        );

        // And the signature is honoured: the address announced is the address served.
        let (host, target, _query) = parts_of(&url);
        assert_eq!(
            host,
            format!("api.ecr.{region}.amazonaws.com"),
            "case {case:?}: the URL names the endpoint the call was made against, got \
             {url:?}"
        );
        let fetched = fetch(&authenticated, "GET", &target, &host).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the announced address must serve the layer, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.bytes.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the {} pushed bytes, got {}",
            content.len(),
            fetched.bytes.len()
        );
    }
}

/// RULE 2: the download plane serves exactly the address it signed.
///
/// The rows differ along what the rule generalises over: WHICH part of the signed address
/// was rewritten (each of the six signing fields in turn, the path, the host, a field of
/// the client's own) and, for the window, every in-range value a client could put there —
/// one second, a minute, the window the registry mints, and the seven days that is the
/// longest a pre-signed URL may carry at all (AWS's published bound on `X-Amz-Expires`,
/// 1..604800), plus one second past it. The controls are the address as announced, the
/// same address with its query fields re-ordered (SigV4's canonical query string is
/// sorted, so order is not part of what is signed) and a `HEAD` of it.
#[tokio::test]
async fn the_download_plane_honours_only_the_address_it_signed() {
    let content = layer_bytes(5, 2_048);
    let (router, engine) = emulator_with_engine();
    let authenticated = authenticated_over(&engine);
    let url = announced_url(&router, "signed addresses", "signedaddresses", &content).await;
    let (target, query) = target_and_query(&url);
    let path = target.split('?').next().unwrap_or_default().to_string();

    // One hex digit of the signature rewritten: still 64 hex characters, still this
    // plane's shape, and not the signature of this address.
    let signature = field(&url, "X-Amz-Signature");
    let flipped: String = signature
        .chars()
        .enumerate()
        .map(|(index, digit)| if index == 0 { flip(digit) } else { digit })
        .collect();
    // The query re-ordered, which a client that re-encodes what it was handed presents.
    let reordered = {
        let mut pairs: Vec<&str> = query.split('&').collect();
        pairs.reverse();
        format!("{path}?{}", pairs.join("&"))
    };
    // Another layer of the same repository, so the retargeted address below names content
    // this registry really holds — what refuses it is the signature, not the absence.
    let other = layer_bytes(7, 64);
    let other_digest = given_layer(&router, "another layer", "signedaddresses", &other).await;
    let retargeted = {
        let (_prefix, rest) = path
            .split_once(&format!("{DOWNLOAD_PATH_PREFIX}/"))
            .expect("the announced path is published under the download prefix");
        let token = rest.split('/').next().unwrap_or_default();
        format!("{DOWNLOAD_PATH_PREFIX}/{token}/{other_digest}?{query}")
    };

    // (case, the address presented, the host it is presented on, the status, the code)
    let cases: Vec<(String, String, &str, u16, &str)> = vec![
        (
            "the address as announced".to_string(),
            target.clone(),
            SERVICE_ENDPOINT,
            200,
            "",
        ),
        (
            "the same address with its query fields re-ordered".to_string(),
            reordered,
            SERVICE_ENDPOINT,
            200,
            "",
        ),
        (
            "no query string at all".to_string(),
            path.clone(),
            SERVICE_ENDPOINT,
            403,
            "AccessDenied",
        ),
        (
            "the signature field removed".to_string(),
            without_field(&url, "X-Amz-Signature"),
            SERVICE_ENDPOINT,
            403,
            "AccessDenied",
        ),
        (
            "one digit of the signature rewritten".to_string(),
            with_field(&url, "X-Amz-Signature", &flipped),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the window shortened to one second".to_string(),
            with_field(&url, "X-Amz-Expires", "1"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the window shortened to a minute".to_string(),
            with_field(&url, "X-Amz-Expires", "60"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the window widened to the longest a pre-signed URL may carry".to_string(),
            with_field(&url, "X-Amz-Expires", "604800"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the window widened past that bound".to_string(),
            with_field(&url, "X-Amz-Expires", "604801"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the window emptied".to_string(),
            with_field(&url, "X-Amz-Expires", ""),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the window field removed".to_string(),
            without_field(&url, "X-Amz-Expires"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the signing timestamp moved".to_string(),
            with_field(&url, "X-Amz-Date", "20260101T000000Z"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the algorithm rewritten".to_string(),
            with_field(&url, "X-Amz-Algorithm", "AWS4-HMAC-SHA1"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the credential scope's Region rewritten".to_string(),
            with_field(
                &url,
                "X-Amz-Credential",
                &field(&url, "X-Amz-Credential").replace("eu-west-1", "us-east-1"),
            ),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the credential scope's signing key rewritten".to_string(),
            with_field(
                &url,
                "X-Amz-Credential",
                &format!("AKIAFORGEDKEY0000000%2F{}", scope_tail(&url)),
            ),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the signed-header list emptied".to_string(),
            with_field(&url, "X-Amz-SignedHeaders", ""),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "a query field of the client's own appended".to_string(),
            format!("{target}&response-content-type=text%2Fplain"),
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "this signature over another layer's address".to_string(),
            retargeted,
            SERVICE_ENDPOINT,
            403,
            "SignatureDoesNotMatch",
        ),
        (
            "the address followed through another endpoint".to_string(),
            target.clone(),
            "127.0.0.1:9297",
            403,
            "SignatureDoesNotMatch",
        ),
    ];

    for (case, presented, host, status, code) in cases {
        let fetched = fetch(&authenticated, "GET", &presented, host).await;
        assert_eq!(
            fetched.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.code(),
            code,
            "case {case:?}: expected the code {code:?}, got {:?}",
            fetched.body
        );
        if code == "SignatureDoesNotMatch" {
            assert_eq!(
                fetched.message(),
                "The request signature we calculated does not match the signature you \
                 provided. Check your key and signing method.",
                "case {case:?}: expected the signature-mismatch wording, got {:?}",
                fetched.body
            );
        }
        if status == 200 {
            assert_eq!(
                fetched.bytes.as_ref(),
                content.as_slice(),
                "case {case:?}: an honoured address serves the layer's own bytes, got {} \
                 bytes",
                fetched.bytes.len()
            );
        } else {
            assert_ne!(
                fetched.bytes.as_ref(),
                content.as_slice(),
                "case {case:?}: a refused fetch serves no layer byte"
            );
        }
    }

    // A `HEAD` of the signed address reports what a `GET` of it would answer about the
    // layer, and is checked against that same signature.
    let head = fetch(&authenticated, "HEAD", &target, SERVICE_ENDPOINT).await;
    assert_eq!(
        head.status, 200,
        "a HEAD of the signed address must be served, got {} with {:?}",
        head.status, head.body
    );
    assert!(
        head.bytes.is_empty(),
        "a HEAD carries no body, got {} bytes",
        head.bytes.len()
    );
}

/// RULE 2, the window half: an address this registry signed stops being honoured once the
/// window its signature covers has closed, and that is a DIFFERENT answer from an address
/// it never signed.
///
/// The rows differ along where the signed window sits relative to the clock: open, one
/// second closed, a minute closed and a day closed. Each address is minted through the
/// service's own signing path at an instant of this suite's choosing, because no
/// reasonable number of wire calls waits a window out — the signature is this registry's
/// either way.
#[tokio::test]
async fn an_address_is_honoured_only_inside_the_window_its_signature_covers() {
    let content = layer_bytes(11, 333);
    let (router, engine) = emulator_with_engine();
    let authenticated = authenticated_over(&engine);
    let url = announced_url(&router, "signed windows", "signedwindows", &content).await;
    let (target, _query) = target_and_query(&url);
    let path = target.split('?').next().unwrap_or_default().to_string();

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
        let signed_at = match closed_for {
            None => SystemTime::now(),
            Some(closed_for) => SystemTime::now() - state::LAYER_DOWNLOAD_URL_TTL - closed_for,
        };
        let signed = state::presigned_layer_download_url(
            SERVICE_ENDPOINT,
            &path,
            REGION,
            signed_at,
            state::LAYER_DOWNLOAD_URL_TTL,
        );
        let fetched = fetch(&authenticated, "GET", &signed, SERVICE_ENDPOINT).await;
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
                    "case {case:?}: an expired address is refused as expired, not as \
                     unsigned, got {:?}",
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
            }
        }
    }
}

/// One hex digit rewritten to another: `0` becomes `1`, anything else becomes `0`.
fn flip(digit: char) -> char {
    if digit == '0' {
        '1'
    } else {
        '0'
    }
}

/// Everything after the signing key in a URL's credential scope, as the query field writes
/// it (`<date>%2F<region>%2F<name>%2Faws4_request`).
fn scope_tail(url: &str) -> String {
    field(url, "X-Amz-Credential")
        .split_once("%2F")
        .map(|(_key, tail)| tail.to_string())
        .unwrap_or_default()
}
