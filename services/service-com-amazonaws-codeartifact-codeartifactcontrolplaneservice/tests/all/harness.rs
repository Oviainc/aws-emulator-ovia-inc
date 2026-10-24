// SPDX-License-Identifier: CC0-1.0

//! The HTTP plumbing every suite in this crate's `tests/all/` shares.
//!
//! Each suite asserts WIRE behaviour: it drives real `restJson1` requests through the
//! `MultiServiceRouter` the production emulator serves with, rather than calling a
//! handler directly. Building that router, the signing-scope header and the response
//! reduction is the same work for every suite, so it lives here once and each suite
//! keeps only the requests and the rules it is about.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header for `region`, in the shape the router's routing key
/// reader parses: its credential scope is what names the target service AND the region
/// every minted ARN carries. The signature itself is never checked here (the harness
/// turns request authentication off), so it is a placeholder.
pub fn auth_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/codeartifact/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// The account id every generated emulator presents, and therefore the `owner`,
/// `domainOwner` and `administratorAccount` of every resource a suite creates.
pub const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region every request is signed for unless a suite names another; every ARN a
/// suite asserts is regional on it.
pub const REGION: &str = "us-east-1";

/// The characters an ARN resource path carries LITERALLY inside a segment: RFC 3986's
/// unreserved set, `ALPHA / DIGIT / "-" / "." / "_" / "~"`.
const ARN_LITERAL_PUNCTUATION: [char; 4] = ['-', '.', '_', '~'];

/// The ARN a package group of `pattern` in `domain` reports:
/// `arn:aws:codeartifact:<region>:<account>:package-group/<domain><pattern>`, with each
/// SEGMENT of the path percent-encoded (lower-case escapes) and the `/` separators kept
/// literal.
///
/// `tests/all/package_group_arn_encoding.rs` pins that encoding on a hand-written table of
/// patterns and the exact ARN each must get; this helper is the same rule in closed form,
/// so a suite that just needs "the ARN of this group" does not restate it.
pub fn package_group_arn(domain: &str, pattern: &str) -> String {
    let path = pattern.strip_prefix('/').unwrap_or(pattern);
    let resource: Vec<String> = format!("{domain}/{path}")
        .split('/')
        .map(|segment| {
            segment
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric()
                        || ARN_LITERAL_PUNCTUATION.contains(&character)
                    {
                        character.to_string()
                    } else {
                        let mut buffer = [0_u8; 4];
                        character
                            .encode_utf8(&mut buffer)
                            .bytes()
                            .map(|byte| format!("%{byte:02x}"))
                            .collect()
                    }
                })
                .collect()
        })
        .collect();
    format!(
        "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package-group/{}",
        resource.join("/")
    )
}

/// One HTTP answer, reduced to what a client branches on.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    /// `x-amzn-errortype`, which is how restJson1 names the error shape.
    pub error_type: Option<String>,
    pub body: Value,
}

impl Answer {
    /// The `__type`/`x-amzn-errortype` error code, or `""` on a success.
    pub fn code(&self) -> String {
        self.error_type.clone().unwrap_or_default()
    }

    /// The `message` of an error body, or `""` when the body carries none.
    pub fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `packageGroup` object of a successful package-group answer.
    pub fn group(&self) -> &Value {
        self.body
            .get("packageGroup")
            .unwrap_or_else(|| panic!("no packageGroup in {:?}", self.body))
    }
}

/// The emulator under test: the production router with this one service mounted on a
/// single shared engine, exactly as `core/router` assembles it.
pub fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// [`emulator`], handing back the shared engine as well — for a suite that reads the
/// stored record directly, to catch a call that mutated state its answer looked right
/// about.
pub fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Deterministic regardless of the ambient `--auth` setting: these requests carry an
    // unsigned placeholder credential, and signature verification is the emulator's job,
    // not this service's.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Send one request signed for [`REGION`] and reduce the response to an [`Answer`].
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    call_in_region(router, REGION, method, uri, body).await
}

/// [`call`], signed for (and addressed to) an arbitrary `region`.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("codeartifact.{region}.amazonaws.com"))
        .header("authorization", auth_for(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({ "unparsed": String::from_utf8_lossy(&raw) }))
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create a domain — the container every other CodeArtifact resource lives in, so every
/// suite starts with one. `encryption_key` is sent only when a suite pins one.
pub async fn create_domain(
    router: &MultiServiceRouter,
    domain: &str,
    encryption_key: Option<&str>,
) -> Answer {
    let body = match encryption_key {
        Some(key) => json!({ "encryptionKey": key }),
        None => json!({}),
    };
    call(
        router,
        "POST",
        &format!("/v1/domain?domain={domain}"),
        Some(body),
    )
    .await
}
