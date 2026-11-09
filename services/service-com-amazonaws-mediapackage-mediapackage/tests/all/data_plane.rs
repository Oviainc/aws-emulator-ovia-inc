// SPDX-License-Identifier: CC0-1.0

//! The two data planes a MediaPackage channel publishes URLs for, end to end: the HLS
//! INGEST endpoints an encoder pushes to, and the PLAYBACK path an origin endpoint is
//! watched at.
//!
//! RULES under test, each driven through the URLs the control plane itself published:
//!
//! 1. An ingest endpoint is a WebDAV collection authenticated with HTTP Digest over the
//!    credentials `CreateChannel` generated for THAT endpoint: a request presenting none is
//!    answered `401` with a `WWW-Authenticate: Digest realm="WebDAV"` challenge, a request
//!    presenting any other scheme is refused `403` with no challenge (so a right and a
//!    wrong credential of the wrong scheme are indistinguishable), a Digest response that
//!    does not verify is `401`, and a verified push is accepted.
//! 2. What a verified push stores is the CHANNEL's media: either endpoint of the redundant
//!    pair feeds the one channel, and its origin endpoints serve it at the playback URLs
//!    they published.
//! 3. An origin endpoint's playback URL answers `403` while its stored `Origination` is
//!    `DENY`, and serves again once it is `ALLOW`.
//! 4. An origin endpoint's playback URL answers `403` for a caller whose source address
//!    lies in none of its stored `Whitelist` blocks, and serves every caller while that
//!    list is empty.
//! 5. An ingest or playback address this service never published is routed to nothing:
//!    `404`, not a refusal class of either plane.
//!
//! Every suite dials the published URL two ways — as published, and at the emulator's own
//! endpoint with the published PATH, which is what a client that resolves no AWS hostname
//! does — because the plane is addressed by that path.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A fresh emulator holding only this service, with request authentication off (this suite
/// exercises behaviour, not signing).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The Region every case below is served in.
const REGION: &str = "us-west-2";

/// A SigV4 `Authorization` whose credential scope routes a CONTROL-plane request to
/// `mediapackage` in [`REGION`]; the signature is never verified here.
fn control_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/mediapackage/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One control-plane round trip: the HTTP status and the parsed JSON body.
async fn control(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map(ToString::to_string).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://mediapackage.{REGION}.amazonaws.com{uri}"))
        .header("authorization", control_authorization())
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One answer of a data plane: the status, the headers and the bytes.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    /// The value of `name`, when the answer carries it.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body as text, for an assertion message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One data-plane round trip to `url`, with `headers` and `body`.
async fn plane(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Answer {
    let mut request = http::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        request = request.header(*name, value.clone());
    }
    let request = request
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).to_string(),
            )
        })
        .collect();
    Answer {
        status,
        headers,
        body: response.into_body().to_vec(),
    }
}

/// The path of an `https://host/path` URL.
fn path_of(url: &str) -> &str {
    url.strip_prefix("https://")
        .and_then(|rest| rest.find('/').map(|start| &rest[start..]))
        .unwrap_or("/")
}

/// How a case ADDRESSES a published URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressing {
    /// The URL exactly as the control plane published it.
    AsPublished,
    /// The emulator's own endpoint, carrying the published path — what a client that
    /// resolves no AWS hostname reaches.
    AtTheEmulatorEndpoint,
}

impl Addressing {
    /// `url` as this addressing dials it.
    fn dial(self, url: &str) -> String {
        match self {
            Self::AsPublished => url.to_string(),
            Self::AtTheEmulatorEndpoint => format!("https://127.0.0.1:9297{}", path_of(url)),
        }
    }

    /// Both addressings, for a case that must answer the same either way.
    fn both() -> [Self; 2] {
        [Self::AsPublished, Self::AtTheEmulatorEndpoint]
    }
}

// ---------------------------------------------------------------------------
// The client side of the two authentication schemes an encoder tries
// ---------------------------------------------------------------------------

/// `value` digested with MD5, hexadecimal, as RFC 2617 writes every digest in.
fn md5_hex(value: &str) -> String {
    use md5::Digest as _;
    hex::encode(md5::Md5::digest(value.as_bytes()))
}

/// The value of the challenge parameter `name`, unquoted.
///
/// A challenge is `<scheme> <parameters>`, so the scheme token is dropped before the
/// parameters are read off it.
fn challenge_parameter(challenge: &str, name: &str) -> String {
    let parameters = challenge
        .trim()
        .split_once(char::is_whitespace)
        .map(|(_scheme, parameters)| parameters)
        .unwrap_or_default();
    parameters
        .split(',')
        .filter_map(|parameter| parameter.trim().split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().trim_matches('"').to_string())
        .unwrap_or_default()
}

/// The `Authorization: Digest` header a client answers `challenge` with for `method` on
/// `path`, computed as RFC 2617 §3.2.2 defines it — the client half of the exchange.
fn digest_authorization(
    challenge: &str,
    method: &str,
    path: &str,
    username: &str,
    password: &str,
) -> String {
    let realm = challenge_parameter(challenge, "realm");
    let nonce = challenge_parameter(challenge, "nonce");
    let qop = challenge_parameter(challenge, "qop");
    let count = "00000001";
    let client_nonce = "0a4f113b42";
    let secret = md5_hex(&format!("{username}:{realm}:{password}"));
    let over_request = md5_hex(&format!("{method}:{path}"));
    let response = md5_hex(&format!(
        "{secret}:{nonce}:{count}:{client_nonce}:{qop}:{over_request}"
    ));
    format!(
        "Digest username=\"{username}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{path}\", \
         algorithm=MD5, qop={qop}, nc={count}, cnonce=\"{client_nonce}\", \
         response=\"{response}\""
    )
}

/// The `Authorization: Basic` header an encoder that tries HTTP Basic sends.
fn basic_authorization(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        aws_smithy_types::base64::encode(format!("{username}:{password}"))
    )
}

/// One ingest endpoint of a channel, as `CreateChannel` published it.
#[derive(Debug, Clone)]
struct Ingest {
    url: String,
    username: String,
    password: String,
}

/// The ingest endpoints of a freshly created channel `id`.
async fn channel_with_ingest(router: &MultiServiceRouter, id: &str) -> Vec<Ingest> {
    let (status, created) = control(router, "POST", "/channels", Some(&json!({ "id": id }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel {id:?} expected 200, got {status} {created}"
    );
    let endpoints = created
        .pointer("/hlsIngest/ingestEndpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        endpoints.len(),
        state::INGEST_ENDPOINT_COUNT,
        "CreateChannel {id:?} must publish the redundant ingest pair, got {created}"
    );
    endpoints
        .iter()
        .map(|endpoint| {
            let text = |member: &str| {
                endpoint
                    .get(member)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Ingest {
                url: text("url"),
                username: text("username"),
                password: text("password"),
            }
        })
        .collect()
}

/// Push `content` under `name` into the channel the ingest endpoint `ingest` belongs to,
/// answering the challenge the plane issues — the whole Digest exchange an encoder makes.
async fn push(
    router: &MultiServiceRouter,
    ingest: &Ingest,
    name: &str,
    content: &[u8],
    password: &str,
) -> Answer {
    let url = format!("{}/{name}", ingest.url);
    let probe = plane(router, "PUT", &url, &[], content).await;
    assert_eq!(
        probe.status, 401,
        "a push with no credentials must be challenged, got {} {}",
        probe.status,
        probe.text()
    );
    let challenge = probe
        .header("www-authenticate")
        .expect("the challenge the 401 carries")
        .to_string();
    let authorization = digest_authorization(
        &challenge,
        "PUT",
        path_of(&url),
        &ingest.username,
        password,
    );
    plane(
        router,
        "PUT",
        &url,
        &[("authorization", authorization)],
        content,
    )
    .await
}

/// Create an origin endpoint on `channel` and answer the playback URL it publishes.
async fn origin_endpoint(
    router: &MultiServiceRouter,
    channel: &str,
    id: &str,
    extra: Value,
) -> String {
    let mut request = json!({ "channelId": channel, "id": id });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    let (status, created) = control(router, "POST", "/origin_endpoints", Some(&request)).await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint {id:?} expected 200, got {status} {created}"
    );
    created
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The playback path prefix an endpoint's own `url` sits under: everything before its
/// manifest's own last segment(s).
fn playback_prefix(url: &str, manifest_tail: &str) -> String {
    url.strip_suffix(manifest_tail)
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_string()
}

// ---------------------------------------------------------------------------
// RULE 1 — what the ingest plane does with the credentials a push presents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_ingest_plane_challenges_digest_and_refuses_every_other_scheme() {
    let router = emulator();
    let ingests = channel_with_ingest(&router, "ingest-auth").await;

    // The rows span what the rule generalises over: which endpoint of the redundant pair
    // the push presents itself at, the path under the published URL (the URL itself and two
    // names below it), and how the URL is addressed.
    for (index, ingest) in ingests.iter().enumerate() {
        for tail in ["", "/index.m3u8", "/720p/seg1.ts"] {
            for addressing in Addressing::both() {
                let url = addressing.dial(&format!("{}{tail}", ingest.url));
                let case = format!("endpoint {index}, tail {tail:?}, {addressing:?}");

                // No credentials at all: the challenge a Digest client is driven by.
                let answer = plane(&router, "PUT", &url, &[], b"#EXTM3U\n").await;
                assert_eq!(
                    answer.status, 401,
                    "case {case:?}: expected 401 for a push with no credentials, got {} {}",
                    answer.status,
                    answer.text()
                );
                let challenge = answer.header("www-authenticate").unwrap_or_default();
                for required in ["Digest", "realm=\"WebDAV\"", "nonce=", "algorithm=MD5"] {
                    assert!(
                        challenge.contains(required),
                        "case {case:?}: the challenge must advertise {required:?}, got \
                         {challenge:?}"
                    );
                }

                // Any other scheme — including HTTP Basic carrying the endpoint's OWN
                // credentials — draws the same bare refusal, with no challenge.
                let wrong_scheme = [
                    ("the endpoint's own credentials", ingest.password.clone()),
                    ("a wrong password", "not-the-password".to_string()),
                ];
                for (presented, password) in wrong_scheme {
                    let answer = plane(
                        &router,
                        "PUT",
                        &url,
                        &[(
                            "authorization",
                            basic_authorization(&ingest.username, &password),
                        )],
                        b"#EXTM3U\n",
                    )
                    .await;
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: expected 403 for HTTP Basic with {presented}, got {} {}",
                        answer.status,
                        answer.text()
                    );
                    assert_eq!(
                        answer.header("www-authenticate"),
                        None,
                        "case {case:?}: the bare refusal of another scheme carries no challenge, \
                         got {:?}",
                        answer.header("www-authenticate")
                    );
                    assert!(
                        answer.body.is_empty(),
                        "case {case:?}: the refusal is bare, got {:?}",
                        answer.text()
                    );
                }

                // A Digest response computed with a wrong password is refused, and
                // challenged again.
                let wrong = digest_authorization(
                    challenge,
                    "PUT",
                    path_of(&url),
                    &ingest.username,
                    "not-the-password",
                );
                let answer = plane(
                    &router,
                    "PUT",
                    &url,
                    &[("authorization", wrong)],
                    b"#EXTM3U\n",
                )
                .await;
                assert_eq!(
                    answer.status, 401,
                    "case {case:?}: expected 401 for a Digest response computed with a wrong \
                     password, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert!(
                    answer
                        .header("www-authenticate")
                        .is_some_and(|value| value.contains("realm=\"WebDAV\"")),
                    "case {case:?}: a 401 carries the challenge, got {:?}",
                    answer.header("www-authenticate")
                );

                // The response this endpoint's own credentials compute is accepted: a
                // named object is stored, and the collection itself takes no write.
                let right = digest_authorization(
                    challenge,
                    "PUT",
                    path_of(&url),
                    &ingest.username,
                    &ingest.password,
                );
                let answer = plane(
                    &router,
                    "PUT",
                    &url,
                    &[("authorization", right)],
                    b"#EXTM3U\n",
                )
                .await;
                let expected = if tail.is_empty() { 405 } else { 200 };
                assert_eq!(
                    answer.status, expected,
                    "case {case:?}: expected {expected} for a verified push, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_ne!(
                    answer.status, 401,
                    "case {case:?}: a verified push must never be challenged"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_rotated_ingest_credential_is_the_only_one_that_authenticates() {
    let router = emulator();
    let ingests = channel_with_ingest(&router, "ingest-rotate").await;
    let before = ingests
        .first()
        .cloned()
        .expect("the first ingest endpoint of the pair");
    let id = path_of(&before.url)
        .trim_end_matches("/channel")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    let accepted = push(&router, &before, "index.m3u8", b"#EXTM3U\n", &before.password).await;
    assert_eq!(
        accepted.status, 200,
        "the credentials CreateChannel published must authenticate, got {} {}",
        accepted.status,
        accepted.text()
    );

    let (status, rotated) = control(
        &router,
        "PUT",
        &format!("/channels/ingest-rotate/ingest_endpoints/{id}/credentials"),
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "RotateIngestEndpointCredentials expected 200, got {status} {rotated}"
    );
    let after = rotated
        .pointer("/hlsIngest/ingestEndpoints")
        .and_then(Value::as_array)
        .and_then(|endpoints| {
            endpoints
                .iter()
                .find(|endpoint| endpoint.get("id").and_then(Value::as_str) == Some(id.as_str()))
                .cloned()
        })
        .expect("the rotated endpoint");
    let after = Ingest {
        url: before.url.clone(),
        username: after
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        password: after
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };

    // The rows span the rule: the credential the rotation replaced no longer
    // authenticates, and the one it minted does.
    let rows: &[(&str, &Ingest, &str, u16)] = &[
        ("the replaced password", &after, &before.password, 401),
        ("the replaced username", &before, &before.password, 401),
        ("the minted credential", &after, &after.password, 200),
    ];
    for (case, ingest, password, expected) in rows {
        let url = format!("{}/index.m3u8", ingest.url);
        let probe = plane(&router, "PUT", &url, &[], b"#EXTM3U\n").await;
        let challenge = probe.header("www-authenticate").unwrap_or_default();
        let authorization = digest_authorization(
            challenge,
            "PUT",
            path_of(&url),
            &ingest.username,
            password,
        );
        let answer = plane(
            &router,
            "PUT",
            &url,
            &[("authorization", authorization)],
            b"#EXTM3U\n",
        )
        .await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected {expected}, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2 — what a channel ingests is what its origin endpoints serve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_origin_endpoint_serves_at_its_published_urls_what_its_channel_ingested() {
    let router = emulator();
    let ingests = channel_with_ingest(&router, "serve-ch").await;
    let manifest = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:6.0,\nseg1.ts\n";
    let segment = b"\x47\x40\x11\x10segment-bytes";

    // Either endpoint of the redundant pair feeds the ONE channel: the playlist goes in
    // through the first, the segment through the second.
    let first = ingests.first().cloned().expect("the first ingest endpoint");
    let second = ingests.get(1).cloned().expect("the second ingest endpoint");
    for (ingest, name, content) in [
        (&first, "index.m3u8", manifest.to_vec()),
        (&second, "seg1.ts", segment.to_vec()),
    ] {
        let answer = push(&router, ingest, name, &content, &ingest.password).await;
        assert_eq!(
            answer.status, 200,
            "pushing {name:?} expected 200, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // The rows span what the rule generalises over: the packaging format, whether the
    // endpoint names a manifest of its own, a child manifest of a CMAF package, the path
    // asked for (the published manifest, a segment, a name nothing was pushed under), and
    // how the URL is addressed.
    struct Row {
        case: &'static str,
        id: &'static str,
        extra: Value,
        manifest_tail: &'static str,
    }
    let rows = vec![
        Row {
            case: "an HLS endpoint taking the default manifest name",
            id: "serve-hls",
            extra: json!({ "hlsPackage": {} }),
            manifest_tail: "/index.m3u8",
        },
        Row {
            case: "an HLS endpoint with a manifest name of its own",
            id: "serve-named",
            extra: json!({ "hlsPackage": {}, "manifestName": "stream" }),
            manifest_tail: "/stream.m3u8",
        },
        Row {
            case: "a CMAF endpoint, whose manifest is a playlist too",
            id: "serve-cmaf",
            extra: json!({ "cmafPackage": { "hlsManifests": [{ "id": "a" }] } }),
            manifest_tail: "/index.m3u8",
        },
    ];
    for row in rows {
        let url = origin_endpoint(&router, "serve-ch", row.id, row.extra.clone()).await;
        let prefix = playback_prefix(&url, row.manifest_tail);
        for addressing in Addressing::both() {
            let case = format!("{}, {addressing:?}", row.case);

            // The endpoint's own manifest URL answers with the manifest its channel is
            // carrying, under the type an HLS playlist is registered as.
            let answer = plane(&router, "GET", &addressing.dial(&url), &[], b"").await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 at the published manifest URL, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.body, manifest,
                "case {case:?}: expected the ingested playlist, got {:?}",
                answer.text()
            );
            assert_eq!(
                answer.header("content-type"),
                Some("application/vnd.apple.mpegurl"),
                "case {case:?}: expected the HLS playlist media type, got {:?}",
                answer.header("content-type")
            );

            // A segment path under the endpoint answers with the bytes pushed under that
            // name, and a name nothing was pushed under is served by nothing.
            let answer = plane(
                &router,
                "GET",
                &addressing.dial(&format!("{prefix}/seg1.ts")),
                &[],
                b"",
            )
            .await;
            assert_eq!(
                (answer.status, answer.header("content-type")),
                (200, Some("video/mp2t")),
                "case {case:?}: expected the ingested segment as video/mp2t, got {} {:?}",
                answer.status,
                answer.header("content-type")
            );
            assert_eq!(
                answer.body, segment,
                "case {case:?}: expected the ingested segment bytes"
            );

            let answer = plane(
                &router,
                "GET",
                &addressing.dial(&format!("{prefix}/seg9.ts")),
                &[],
                b"",
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404 for a name nothing was pushed under, got {} {}",
                answer.status,
                answer.text()
            );

            // A HEAD is the same answer without the body.
            let answer = plane(&router, "HEAD", &addressing.dial(&url), &[], b"").await;
            assert_eq!(
                (answer.status, answer.header("content-length")),
                (200, Some(manifest.len().to_string().as_str())),
                "case {case:?}: a HEAD states the length of the manifest, got {} {:?}",
                answer.status,
                answer.header("content-length")
            );
            assert!(
                answer.body.is_empty(),
                "case {case:?}: a HEAD carries no body, got {:?}",
                answer.text()
            );

            // A playback path takes no writes.
            let answer = plane(&router, "PUT", &addressing.dial(&url), &[], b"x").await;
            assert_eq!(
                (answer.status, answer.header("allow")),
                (405, Some("GET, HEAD")),
                "case {case:?}: expected 405 naming the methods playback serves, got {} {:?}",
                answer.status,
                answer.header("allow")
            );
        }
    }

    // A CMAF package's CHILD manifest URL is published too, and is served by the same
    // channel's playlist.
    let (status, described) = control(&router, "GET", "/origin_endpoints/serve-cmaf", None).await;
    assert_eq!(
        status, 200,
        "DescribeOriginEndpoint expected 200, got {status} {described}"
    );
    let child = described
        .pointer("/cmafPackage/hlsManifests/0/url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let answer = plane(&router, "GET", &child, &[], b"").await;
    assert_eq!(
        (answer.status, answer.body.clone()),
        (200, manifest.to_vec()),
        "a published child manifest URL serves the channel's playlist, got {} {}",
        answer.status,
        answer.text()
    );

    // An endpoint whose channel was pushed nothing of its format has nothing to serve,
    // which is a 404 and not a refusal: the DASH manifest of this channel does not exist.
    let dash = origin_endpoint(&router, "serve-ch", "serve-dash", json!({ "dashPackage": {} })).await;
    let answer = plane(&router, "GET", &dash, &[], b"").await;
    assert_eq!(
        answer.status, 404,
        "a format nothing was pushed for serves nothing, got {} {}",
        answer.status,
        answer.text()
    );
}

// ---------------------------------------------------------------------------
// RULE 3 — Origination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn playback_is_refused_while_the_endpoints_origination_denies_it() {
    let router = emulator();
    let ingests = channel_with_ingest(&router, "origination-ch").await;
    let ingest = ingests.first().cloned().expect("an ingest endpoint");
    let pushed = push(
        &router,
        &ingest,
        "index.m3u8",
        b"#EXTM3U\n",
        &ingest.password,
    )
    .await;
    assert_eq!(pushed.status, 200, "the setup push must be accepted");

    // The rows span what the rule generalises over: the packaging format, whether the
    // endpoint has content to serve at all (so the refusal is decided before the content
    // is), and the whole ALLOW -> DENY -> ALLOW toggle on the IDENTICAL URL.
    struct Row {
        case: &'static str,
        id: &'static str,
        package: Value,
        /// What the URL answers while origination is permitted: the manifest when the
        /// channel carries that format, and "nothing here" when it does not.
        allowed: u16,
    }
    let rows = vec![
        Row {
            case: "an HLS endpoint whose channel carries a playlist",
            id: "origination-hls",
            package: json!({ "hlsPackage": {} }),
            allowed: 200,
        },
        Row {
            case: "a DASH endpoint whose channel carries nothing of its format",
            id: "origination-dash",
            package: json!({ "dashPackage": {} }),
            allowed: 404,
        },
    ];
    for row in rows {
        let mut created = row.package.clone();
        created["channelId"] = json!("origination-ch");
        let url = origin_endpoint(&router, "origination-ch", row.id, row.package.clone()).await;

        // (what the endpoint is updated to, what playback answers afterwards)
        let toggles: &[(&str, Option<&str>, u16)] = &[
            ("as created, taking the default", None, row.allowed),
            ("origination denied", Some("DENY"), 403),
            ("origination allowed again", Some("ALLOW"), row.allowed),
            ("denied once more", Some("DENY"), 403),
        ];
        for (toggle, origination, expected) in toggles {
            if let Some(origination) = origination {
                let mut request = row.package.clone();
                request["origination"] = json!(origination);
                let (status, updated) = control(
                    &router,
                    "PUT",
                    &format!("/origin_endpoints/{}", row.id),
                    Some(&request),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?} {toggle:?}: UpdateOriginEndpoint expected 200, got {status} \
                     {updated}",
                    row.case
                );
                assert_eq!(
                    updated.get("origination").and_then(Value::as_str),
                    Some(*origination),
                    "case {:?} {toggle:?}: the update must store {origination:?}, got {updated}",
                    row.case
                );
            }
            for addressing in Addressing::both() {
                let answer = plane(&router, "GET", &addressing.dial(&url), &[], b"").await;
                assert_eq!(
                    answer.status, *expected,
                    "case {:?} {toggle:?} {addressing:?}: expected {expected}, got {} {}",
                    row.case,
                    answer.status,
                    answer.text()
                );
                if *expected == 403 {
                    assert!(
                        answer.body.is_empty(),
                        "case {:?} {toggle:?}: the refusal is bare, got {:?}",
                        row.case,
                        answer.text()
                    );
                }
            }
        }
        assert_eq!(
            created["channelId"], json!("origination-ch"),
            "the row's own request names the channel it was created on"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4 — Whitelist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn playback_is_refused_for_a_source_address_outside_the_whitelist() {
    let router = emulator();
    let ingests = channel_with_ingest(&router, "whitelist-ch").await;
    let ingest = ingests.first().cloned().expect("an ingest endpoint");
    let pushed = push(
        &router,
        &ingest,
        "index.m3u8",
        b"#EXTM3U\n",
        &ingest.password,
    )
    .await;
    assert_eq!(pushed.status, 200, "the setup push must be accepted");

    // A sibling endpoint on the same channel with NO whitelist, which no case restricts.
    let open = origin_endpoint(
        &router,
        "whitelist-ch",
        "whitelist-open",
        json!({ "hlsPackage": {} }),
    )
    .await;
    let url = origin_endpoint(
        &router,
        "whitelist-ch",
        "whitelist-gated",
        json!({ "hlsPackage": {} }),
    )
    .await;

    // The rows span what the rule generalises over: no list at all, a single-host block, a
    // wider block, several blocks, both address families, a block written with its host
    // bits set, and the list cleared again — each against a caller inside it and a caller
    // outside it. The caller's address is the first hop a forwarder recorded, and the
    // loopback address for a request no forwarder recorded one for.
    struct Row {
        case: &'static str,
        whitelist: Vec<&'static str>,
        /// (the address the caller comes from, or `None` for no forwarded address at all,
        /// the status playback answers)
        callers: Vec<(Option<&'static str>, u16)>,
    }
    let rows = vec![
        Row {
            case: "no whitelist at all",
            whitelist: vec![],
            callers: vec![(None, 200), (Some("203.0.113.9"), 200)],
        },
        Row {
            case: "a documentation block that is never the caller's address",
            whitelist: vec!["203.0.113.0/32"],
            callers: vec![
                (None, 403),
                (Some("198.51.100.4"), 403),
                (Some("203.0.113.0"), 200),
            ],
        },
        Row {
            case: "the loopback block the caller is in",
            whitelist: vec!["127.0.0.0/8"],
            callers: vec![(None, 200), (Some("127.0.0.9"), 200), (Some("10.0.0.1"), 403)],
        },
        Row {
            case: "a /24, inside and outside",
            whitelist: vec!["198.51.100.0/24"],
            callers: vec![
                (Some("198.51.100.77"), 200),
                (Some("198.51.101.77"), 403),
                (None, 403),
            ],
        },
        Row {
            case: "several blocks, the second of which admits the caller",
            whitelist: vec!["10.0.0.0/8", "192.0.2.0/24"],
            callers: vec![(Some("192.0.2.5"), 200), (Some("172.16.0.5"), 403)],
        },
        Row {
            case: "an IPv6 block",
            whitelist: vec!["2001:db8::/32"],
            callers: vec![(Some("2001:db8::5"), 200), (Some("2001:db9::5"), 403)],
        },
        Row {
            case: "a block written with its host bits set",
            whitelist: vec!["10.1.2.3/8"],
            callers: vec![(Some("10.9.9.9"), 200), (Some("11.0.0.1"), 403)],
        },
        Row {
            case: "the whitelist cleared again",
            whitelist: vec![],
            callers: vec![(None, 200), (Some("203.0.113.9"), 200)],
        },
    ];
    for row in rows {
        let (status, updated) = control(
            &router,
            "PUT",
            "/origin_endpoints/whitelist-gated",
            Some(&json!({ "hlsPackage": {}, "whitelist": row.whitelist })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateOriginEndpoint expected 200, got {status} {updated}",
            row.case
        );
        assert_eq!(
            updated.get("whitelist").and_then(Value::as_array).map(Vec::len),
            Some(row.whitelist.len()),
            "case {:?}: the update must store the whitelist it was given, got {updated}",
            row.case
        );
        for (caller, expected) in &row.callers {
            let headers: Vec<(&str, String)> = caller
                .map(|address| vec![("x-forwarded-for", address.to_string())])
                .unwrap_or_default();
            let answer = plane(&router, "GET", &url, &headers, b"").await;
            assert_eq!(
                answer.status, *expected,
                "case {:?}, caller {caller:?}: expected {expected}, got {} {}",
                row.case,
                answer.status,
                answer.text()
            );
            // The sibling endpoint with no whitelist of its own is never restricted by
            // this one's.
            let sibling = plane(&router, "GET", &open, &headers, b"").await;
            assert_eq!(
                sibling.status, 200,
                "case {:?}, caller {caller:?}: the unrestricted sibling must still serve, got \
                 {} {}",
                row.case,
                sibling.status,
                sibling.text()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 5 — an address this service never published is routed to nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_data_plane_address_this_service_never_published_is_routed_to_nothing() {
    let router = emulator();
    let ingests = channel_with_ingest(&router, "unpublished-ch").await;
    let ingest = ingests.first().cloned().expect("an ingest endpoint");
    let url = origin_endpoint(
        &router,
        "unpublished-ch",
        "unpublished-oe",
        json!({ "hlsPackage": {} }),
    )
    .await;
    let host = "https://127.0.0.1:9297";
    let label = "0123456789abcdef0123456789abcdef";

    // The rows span what the rule generalises over: an ingest path whose channel label
    // names no channel, one whose endpoint id names no endpoint of a channel that DOES
    // exist, a playback path naming no endpoint, and a path that is shaped like neither.
    let rows: Vec<(&str, String)> = vec![
        (
            "an ingest path naming no channel",
            format!("{host}/in/v2/{label}/{label}/channel/index.m3u8"),
        ),
        (
            "an ingest path naming no endpoint of a channel that exists",
            format!(
                "{host}{}",
                path_of(&ingest.url).replace(
                    path_of(&ingest.url)
                        .trim_end_matches("/channel")
                        .rsplit('/')
                        .next()
                        .unwrap_or_default(),
                    label,
                )
            ),
        ),
        (
            "a playback path naming no endpoint",
            format!("{host}/out/v1/{label}/index.m3u8"),
        ),
        (
            "a path shaped like neither plane",
            format!("{host}/out/v2/{label}/index.m3u8"),
        ),
    ];
    for (case, address) in rows {
        let answer = plane(&router, "GET", &address, &[], b"").await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for an address this service never published, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.header("www-authenticate"),
            None,
            "case {case:?}: an unpublished address is not a credential-gated one"
        );
    }

    // And an endpoint that is DELETED stops serving at the URL it published.
    let (status, deleted) = control(
        &router,
        "DELETE",
        "/origin_endpoints/unpublished-oe",
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "DeleteOriginEndpoint expected 202, got {status} {deleted}"
    );
    let answer = plane(&router, "GET", &url, &[], b"").await;
    assert_eq!(
        answer.status, 404,
        "a deleted endpoint's playback URL serves nothing, got {} {}",
        answer.status,
        answer.text()
    );
}
