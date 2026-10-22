// SPDX-License-Identifier: CC0-1.0

//! An awsJson request whose only service signal is `X-Amz-Target` must still reach
//! the service that target names, in the FULLY assembled router.
//!
//! `resolve_route_key` reads a SigV4 credential scope first and `X-Amz-Target`
//! second, and an `X-Amz-Target` resolves to the service's SHAPE name
//! (`PartnerCentralChannel` -> `partnercentralchannel`), not to its SigV4 signing
//! name (`partnercentral-channel`). A request that carries no readable
//! `AWS4-HMAC-SHA256` scope — because it is unsigned, or signed with SigV4A, which
//! this service's model lists first — therefore routes on that shape-name key. A
//! service matching only its signing name leaves such a request UNCLAIMED, which the
//! router answers `404 UnknownService` in its own awsJson1_1 envelope: the wrong
//! status AND the wrong protocol, from a service that is mounted and implemented.
//!
//! This suite pins both halves in the production router: the request is claimed by
//! EXACTLY one of the hundreds of mounted services (never `AmbiguousService`) and is
//! served by this one in its own awsJson1_0 envelope.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for
//! the same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4A `Authorization` header: the scheme `PartnerCentralChannel`'s model
/// lists FIRST (`aws.auth#sigv4a`). Its algorithm is not the `AWS4-HMAC-SHA256`
/// form the route-key reader parses, so such a request routes on `X-Amz-Target`
/// alone. Request authentication is off in this router, so the signature value
/// itself is never checked.
const SIGV4A_AUTHORIZATION: &str = "AWS4-ECDSA-P256-SHA256 \
     Credential=AKIDEXAMPLE/20260101/partnercentral-channel/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0123456789abcdef";

/// One routing case: how the request addresses the service.
struct Case {
    name: &'static str,
    host: &'static str,
    /// The `Authorization` header, when the request carries one.
    authorization: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        name: "unsigned, addressed by target alone",
        host: "127.0.0.1:9297",
        authorization: None,
    },
    Case {
        name: "SigV4A-signed, addressed by target alone",
        host: "127.0.0.1:9297",
        authorization: Some(SIGV4A_AUTHORIZATION),
    },
];

/// A well-formed `ListChannelHandshakes` body: the three required members, with
/// filters no handshake can match in a freshly built router.
const LIST_BODY: &str = r#"{"catalog":"AWS","handshakeType":"PROGRAM_MANAGEMENT_ACCOUNT","participantType":"SENDER"}"#;

#[test]
fn an_awsjson_target_only_request_reaches_the_service_its_target_names() {
    std::thread::Builder::new()
        .name("partnercentral-channel-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let router = build_router(engine);

                for case in CASES {
                    let mut request = http::Request::builder()
                        .method("POST")
                        .uri(format!("https://{}/", case.host))
                        .header("host", case.host)
                        .header("content-type", "application/x-amz-json-1.0")
                        .header(
                            "x-amz-target",
                            "PartnerCentralChannel.ListChannelHandshakes",
                        );
                    if let Some(authorization) = case.authorization {
                        request = request.header("authorization", authorization);
                    }
                    let request = request
                        .body(bytes::Bytes::from_static(LIST_BODY.as_bytes()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let content_type = response
                        .headers()
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {}: two services claimed one request: {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {}: no service claimed the request: {status} {body}",
                        case.name
                    );
                    assert_eq!(
                        status, 200,
                        "case {}: a list whose filters match nothing is a success, got {status} \
                         {body}",
                        case.name
                    );
                    assert_eq!(
                        content_type, "application/x-amz-json-1.0",
                        "case {}: the model binds this service to awsJson1_0, got \
                         {content_type:?}",
                        case.name
                    );
                    assert!(
                        body.contains("\"items\":[]"),
                        "case {}: items must be serialized as an empty list, got {status} {body}",
                        case.name
                    );
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
