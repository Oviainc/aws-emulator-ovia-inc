// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the member set a layer version's `Content` carries.
//!
//! The rule under test is that every read of a layer version reports its archive through
//! the same `LayerVersionContentOutput` member set AWS carries — the link to the archive,
//! its digest, its stored size and the size of its expanded contents
//! (`UncompressedCodeSize`) — so a client can read any of them off the body without
//! testing for the key. The expanded size is a measurement of a copy Lambda holds
//! unpacked, and a layer archive is stored exactly as it was uploaded, so the member is
//! carried with its own default whatever the archive: a client reading it gets a number
//! rather than nothing.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals, and every
//! published version is read back through `GetLayerVersion` and `GetLayerVersionByArn`,
//! so no assertion trusts the publish alone.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one JSON request, the shape every operation here uses.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Percent-encode the characters an ARN carries that a query string would otherwise read
/// as structure.
fn encoded(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A")
}

/// Layer archive bytes of a given length. The content is irrelevant to the API — only its
/// length and its digest are reported — but it differs per length so two archives never
/// share a digest.
fn archive_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `Content.ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// `PublishLayerVersion` on a layer name, with the given request body.
async fn publish(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetLayerVersion` on a layer name and a version number.
async fn get_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetLayerVersionByArn`, whose `Arn` member is bound to the `Arn` query parameter of
/// `GET /2018-10-31/layers?find=LayerVersion`.
async fn get_version_by_arn(router: &MultiServiceRouter, arn: &str) -> Answer {
    let uri = format!("/2018-10-31/layers?find=LayerVersion&Arn={}", encoded(arn));
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Content` member of a layer-version body.
fn content(case: &str, reader: &str, body: &Value) -> Value {
    body.get("Content")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?} ({reader}): expected a Content member, got {body:?}"))
}

/// The `Content.UncompressedCodeSize` the body carries, which every layer version must
/// report as a number rather than leave out.
fn uncompressed_code_size(case: &str, reader: &str, body: &Value) -> i64 {
    let content = content(case, reader, body);
    let member = content
        .get("UncompressedCodeSize")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?} ({reader}): expected Content to carry UncompressedCodeSize, got \
                 {content:?}"
            )
        });
    member.as_i64().unwrap_or_else(|| {
        panic!("case {case:?} ({reader}): expected UncompressedCodeSize to be a number, got {member:?}")
    })
}

/// The `Content.CodeSize` the body carries: the size of the stored archive itself.
fn code_size(case: &str, reader: &str, body: &Value) -> i64 {
    let content = content(case, reader, body);
    content
        .get("CodeSize")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?} ({reader}): expected a CodeSize number, got {content:?}"))
}

/// One published layer version: how it was published and what its archive measures.
struct Case {
    /// The case name, which is also the layer name.
    name: &'static str,
    /// The `Content` member the version is published with.
    content: Value,
    /// The size of the stored archive, which is what `CodeSize` reports: the length of
    /// the inline bytes, and 0 for an archive this emulator holds no bytes for.
    code_size: i64,
}

/// Every read of a layer version carries the expanded size of its archive, whatever the
/// archive was published from.
#[tokio::test]
async fn every_layer_version_read_carries_the_expanded_size_of_its_archive() {
    // One row per form the rule generalises over: how the archive was supplied (inline
    // bytes or an Amazon S3 location, in both storage modes) and how large the stored
    // archive is (empty, small, large) — the member is carried for each of them, and
    // tracks none of them, because no expanded copy of the archive exists to measure.
    let cases = vec![
        Case {
            name: "inline-empty",
            content: json!({ "ZipFile": zip_file(&[]) }),
            code_size: 0,
        },
        Case {
            name: "inline-small",
            content: json!({ "ZipFile": zip_file(&archive_bytes(64)) }),
            code_size: 64,
        },
        Case {
            name: "inline-large",
            content: json!({ "ZipFile": zip_file(&archive_bytes(4_096)) }),
            code_size: 4_096,
        },
        Case {
            name: "from-s3-copied",
            content: json!({ "S3Bucket": "layer-archives", "S3Key": "helpers.zip" }),
            code_size: 0,
        },
        Case {
            name: "from-s3-referenced",
            content: json!({
                "S3Bucket": "layer-archives",
                "S3Key": "referenced.zip",
                "S3ObjectVersion": "v2",
                "S3ObjectStorageMode": "REFERENCE",
            }),
            code_size: 0,
        },
    ];

    let router = router();
    for case in &cases {
        let published = publish(&router, case.name, &json!({ "Content": case.content })).await;
        assert_eq!(
            published.status,
            201,
            "case {:?}: expected HTTP 201, got {} {}",
            case.name,
            published.status,
            published.text()
        );

        let arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{}:1", case.name);
        let read = get_version(&router, case.name, 1).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected HTTP 200 from GetLayerVersion, got {} {}",
            case.name,
            read.status,
            read.text()
        );
        let by_arn = get_version_by_arn(&router, &arn).await;
        assert_eq!(
            by_arn.status,
            200,
            "case {:?}: expected HTTP 200 from GetLayerVersionByArn, got {} {}",
            case.name,
            by_arn.status,
            by_arn.text()
        );

        // The publish and both reads of one version report the same archive, member for
        // member: each is answered from the stored record rather than from the request.
        for (reader, body) in [
            ("PublishLayerVersion", published.json()),
            ("GetLayerVersion", read.json()),
            ("GetLayerVersionByArn", by_arn.json()),
        ] {
            let expanded = uncompressed_code_size(case.name, reader, &body);
            assert_eq!(
                expanded, 0,
                "case {:?} ({reader}): expected UncompressedCodeSize 0, got {expanded}",
                case.name
            );
            let stored = code_size(case.name, reader, &body);
            assert_eq!(
                stored, case.code_size,
                "case {:?} ({reader}): expected CodeSize {}, got {stored}",
                case.name, case.code_size
            );
        }
    }
}

/// The member is carried by every version of a layer, not only by its first: a second
/// publish under the same name reports it as well, and so does the read of the older
/// version beside it.
#[tokio::test]
async fn every_version_of_one_layer_carries_the_member() {
    let router = router();
    for length in [32_usize, 256] {
        let published = publish(
            &router,
            "versioned",
            &json!({ "Content": { "ZipFile": zip_file(&archive_bytes(length)) } }),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case {length}: expected HTTP 201, got {} {}",
            published.status,
            published.text()
        );
    }

    for version in [1_i64, 2] {
        let body = get_version(&router, "versioned", version).await.json();
        let case = format!("version {version}");
        let expanded = uncompressed_code_size(&case, "GetLayerVersion", &body);
        assert_eq!(
            expanded, 0,
            "case {case:?}: expected UncompressedCodeSize 0, got {expanded}"
        );
    }
}
