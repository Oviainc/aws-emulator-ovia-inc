// SPDX-License-Identifier: CC0-1.0

//! CloudFront's key value store CONTROL plane and its DATA plane are two services, and
//! the only thing that joins them is the store ARN: `CreateKeyValueStore` (signing name
//! `cloudfront`) mints it, and every `cloudfront-keyvaluestore` operation addresses the
//! store by it.
//!
//! So this suite needs the WHOLE router: the handshake only exists when both services
//! are mounted. It asserts the rule that joins them — the ARN the control plane mints
//! for a store is the one identifier under which that store's keys live, and two stores
//! minted by two creates are two separate key spaces.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the CloudFront control plane's signing name, which
/// is what routes a request to it.
const CLOUDFRONT_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudfront/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The same for the key value store DATA plane, whose signing name is its own.
const KVS_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cloudfront-keyvaluestore/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One answer: the status, the `ETag` header and the body as text.
struct Reply {
    status: u16,
    etag: String,
    body: String,
}

/// Send one request through the whole router.
async fn send(
    router: &MultiServiceRouter,
    authorization: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    if_match: Option<&str>,
    body: &str,
) -> Reply {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .header("authorization", authorization)
        .header("content-type", content_type);
    if let Some(version) = if_match {
        builder = builder.header("if-match", version);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8_lossy(response.body()).to_string();
    Reply { status, etag, body }
}

/// The text of one XML element of a restXml answer, or a panic naming the case.
fn element(body: &str, name: &str, case: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("case {case}: no <{name}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("case {case}: no </{name}> in {body}"))
        + start;
    body[start..end].to_string()
}

/// Percent-encode one URI path segment or form component, leaving only RFC 3986's
/// unreserved characters literal. An ARN carries `:` and `/`, both of which a non-greedy
/// `@httpLabel` must arrive escaped — exactly as an AWS SDK sends it; an
/// `application/x-www-form-urlencoded` component is escaped the same way, which is why
/// `all/fis_execution_role_trust.rs` shares this one rather than writing a second copy.
pub(crate) fn segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[test]
fn a_created_key_value_store_is_addressable_by_its_arn_on_the_data_plane() {
    std::thread::Builder::new()
        .name("cloudfront-kvs-handshake".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Each row: the case, the store NAME the control plane creates, and the
            // key and value its own data plane then holds. The rows differ in the name
            // form and in the key and value bytes, so neither store's data can be
            // mistaken for the other's.
            let cases: [(&str, &str, &str, &str); 2] = [
                ("the first store", "handshake_one", "greeting", "hello"),
                ("the second store", "handshake-two", "greeting/nested", "{\"n\":2}"),
            ];

            let mut stores = Vec::new();
            for (case, name, key, value) in cases {
                // The control plane mints the store, and with it the ARN.
                let created = runtime.block_on(send(
                    &router,
                    CLOUDFRONT_AUTHORIZATION,
                    "POST",
                    "/2020-05-31/key-value-store",
                    "application/xml",
                    None,
                    &format!(
                        "<CreateKeyValueStoreRequest \
                         xmlns=\"http://cloudfront.amazonaws.com/doc/2020-05-31/\">\
                         <Name>{name}</Name></CreateKeyValueStoreRequest>"
                    ),
                ));
                assert_eq!(
                    created.status, 201,
                    "case {case}: the control plane must create the store, got {} with {}",
                    created.status, created.body
                );
                let arn = element(&created.body, "ARN", case);

                // The data plane accepts that ARN and holds a key under it.
                let described = runtime.block_on(send(
                    &router,
                    KVS_AUTHORIZATION,
                    "GET",
                    &format!("/key-value-stores/{}", segment(&arn)),
                    "application/json",
                    None,
                    "",
                ));
                assert_eq!(
                    described.status, 200,
                    "case {case}: the data plane must serve the minted ARN {arn}, got {} with {}",
                    described.status, described.body
                );

                let put = runtime.block_on(send(
                    &router,
                    KVS_AUTHORIZATION,
                    "PUT",
                    &format!(
                        "/key-value-stores/{}/keys/{}",
                        segment(&arn),
                        segment(key)
                    ),
                    "application/json",
                    Some(&described.etag),
                    &format!("{{\"Value\":\"{}\"}}", value.replace('"', "\\\"")),
                ));
                assert_eq!(
                    put.status, 200,
                    "case {case}: the put must land in the store the ARN names, got {} with {}",
                    put.status, put.body
                );
                stores.push((case, arn, key, value));
            }

            // Every key reads back from the store whose ARN it was written under, and
            // from no other: the ARN the control plane minted is the key space.
            for (case, arn, key, value) in &stores {
                let got = runtime.block_on(send(
                    &router,
                    KVS_AUTHORIZATION,
                    "GET",
                    &format!("/key-value-stores/{}/keys/{}", segment(arn), segment(key)),
                    "application/json",
                    None,
                    "",
                ));
                assert_eq!(
                    got.status, 200,
                    "case {case}: expected the value written under {arn}, got {} with {}",
                    got.status, got.body
                );
                assert!(
                    got.body.contains(&format!("\"Value\":\"{}\"", value.replace('"', "\\\""))),
                    "case {case}: expected the stored value {value:?}, got {}",
                    got.body
                );

                for (other_case, other_arn, _, _) in &stores {
                    if other_arn == arn {
                        continue;
                    }
                    let crossed = runtime.block_on(send(
                        &router,
                        KVS_AUTHORIZATION,
                        "GET",
                        &format!(
                            "/key-value-stores/{}/keys/{}",
                            segment(other_arn),
                            segment(key)
                        ),
                        "application/json",
                        None,
                        "",
                    ));
                    assert_eq!(
                        crossed.status, 404,
                        "case {case} read through {other_case}: a key belongs to ONE store, \
                         got {} with {}",
                        crossed.status, crossed.body
                    );
                }
            }
        })
        .expect("spawn the handshake thread")
        .join()
        .expect("a created key value store must be addressable on its data plane");
}
