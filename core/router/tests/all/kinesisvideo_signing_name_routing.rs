// SPDX-License-Identifier: CC0-1.0

//! The five Kinesis Video services share one SigV4 signing name — each request must
//! still be claimed by exactly one of them in the assembled router.
//!
//! The control plane (`Kinesis Video`), Media, Archived Media, Signaling Channels and
//! WebRTC Storage all declare `aws.auth#sigv4 name = "kinesisvideo"` and the
//! `kinesisvideo` endpoint prefix, so the routing key alone matches five services and
//! the router rejects a request several services claim with `500 AmbiguousService` —
//! which makes ALL FIVE unreachable. What separates them is their modelled path
//! spaces, which are DISJOINT at the first URI segment, so each crate's `can_handle`
//! adds the framework's path conjunct — `emulator_server::can_handle_based_on_route_key_and_path`
//! over its own generated `ROUTE_PATH_SEGMENTS`, matched by exact segment equality —
//! to the routing-key test. This suite pins that split in the production router: the
//! endpoint host does not matter, the modelled path space does.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for
//! the same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `kinesisvideo` signing name, which is what
/// all five services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request every `kinesisvideo`-signed service could claim on
/// its signing name alone.
enum Expect {
    /// Answered by the service whose model binds the path: the status plus a body
    /// fragment only THAT crate produces is what names the service that served it.
    Answered { status: u16, contains: &'static str },
    /// Bound by NO candidate's modelled path space, so the front door has nothing to
    /// dispatch to and refuses it as `UnknownService` — the one refusal the path
    /// conjunct is allowed to produce, and deliberately NOT `AmbiguousService`.
    NoServiceBindsThePath,
}

/// One routing case.
struct Case {
    name: &'static str,
    host: &'static str,
    path: &'static str,
    body: &'static str,
    expect: Expect,
}

const CASES: &[Case] = &[
    Case {
        name: "control-plane CreateStream on the kinesisvideo endpoint host",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/createStream",
        body: r#"{"StreamName":"routing-probe"}"#,
        expect: Expect::Answered {
            status: 200,
            contains: "StreamARN",
        },
    },
    Case {
        name: "control-plane ListTagsForStream for a stream nobody created",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/listTagsForStream",
        body: r#"{"StreamName":"absent-stream"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
    Case {
        name: "control-plane TagStream for a stream nobody created, on a loopback endpoint",
        host: "127.0.0.1:9297",
        path: "/tagStream",
        body: r#"{"StreamName":"absent-stream","Tags":{"env":"prod"}}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
    Case {
        name: "control-plane UntagStream for a stream nobody created",
        host: "127.0.0.1:9297",
        path: "/untagStream",
        body: r#"{"StreamName":"absent-stream","TagKeyList":["env"]}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
    Case {
        name: "control-plane TagResource with a malformed resourceARN",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/TagResource",
        body: r#"{"ResourceARN":"not-an-arn","Tags":[{"Key":"env","Value":"prod"}]}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "InvalidArgumentException",
        },
    },
    Case {
        // The media service refuses a request naming the stream BOTH ways, in wording
        // only its `GetMedia` produces; the control plane binds no `/getMedia` at all.
        name: "Media GetMedia reaches the media service, not the control plane",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/getMedia",
        body: r#"{"StreamName":"routing-probe","StreamARN":"arn:aws:kinesisvideo:us-east-1:000000000000:stream/routing-probe/1","StartSelector":{"StartSelectorType":"NOW"}}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "Only one of StreamName and StreamARN",
        },
    },
    Case {
        // The archived-media service words a missing stream its own way.
        name: "Archived Media GetHLSStreamingSessionURL reaches the reader service",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/getHLSStreamingSessionURL",
        body: r#"{"StreamName":"absent-stream"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "No such stream: StreamName absent-stream",
        },
    },
    Case {
        name: "Archived Media ListFragments reaches the reader service",
        host: "127.0.0.1:9297",
        path: "/listFragments",
        body: r#"{"StreamName":"absent-stream"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "No such stream: StreamName absent-stream",
        },
    },
    Case {
        // Still a scaffold in the signaling service: the 501 naming the OPERATION is
        // what proves that service claimed the `/v1` path space (the control plane
        // binds none of it, and would not name this operation).
        name: "Signaling Channels GetIceServerConfig reaches the signaling service",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/v1/get-ice-server-config",
        body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:000000000000:channel/probe/1"}"#,
        expect: Expect::Answered {
            status: 501,
            contains: "GetIceServerConfig",
        },
    },
    Case {
        // The WebRTC storage service refuses an ARN that names a stream rather than a
        // signaling channel, in wording only its `JoinStorageSession` produces.
        name: "WebRTC Storage JoinStorageSession reaches the routing service",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/joinStorageSession",
        body: r#"{"channelArn":"arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1"}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "channelArn",
        },
    },
    Case {
        name: "a kinesisvideo path no service binds is refused, not claimed ambiguously",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/notAnOperation",
        body: "{}",
        expect: Expect::NoServiceBindsThePath,
    },
    Case {
        // The claim rule is the first path SEGMENT, so this is still the control
        // plane's path space; its route matcher then reads the trailing slash as the
        // same route, and exactly one service answered.
        name: "a control-plane path with a trailing slash",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/listTagsForStream/",
        body: r#"{"StreamName":"absent-stream"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
];

#[test]
fn every_kinesisvideo_path_is_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("kinesisvideo-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let router = build_router(engine);

                for case in CASES {
                    let request = http::Request::builder()
                        .method("POST")
                        .uri(format!("https://{}{}", case.host, case.path))
                        .header("host", case.host)
                        .header("authorization", AUTHORIZATION)
                        .header("content-type", "application/json")
                        .body(bytes::Bytes::from_static(case.body.as_bytes()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {}: several services claimed one request: {status} {body}",
                        case.name
                    );
                    if !matches!(case.expect, Expect::NoServiceBindsThePath) {
                        assert!(
                            !body.contains("UnknownService"),
                            "case {}: no service claimed the request: {status} {body}",
                            case.name
                        );
                    }
                    match case.expect {
                        Expect::Answered { status: want, contains } => {
                            assert_eq!(
                                status, want,
                                "case {}: the service whose model binds this path must answer \
                                 {want}, got {status} {body}",
                                case.name
                            );
                            assert!(
                                body.contains(contains),
                                "case {}: expected {contains:?} in the body, got {status} {body}",
                                case.name
                            );
                        }
                        Expect::NoServiceBindsThePath => {
                            assert!(
                                body.contains("UnknownService"),
                                "case {}: a path no model binds must be refused as \
                                 UnknownService, got {status} {body}",
                                case.name
                            );
                        }
                    }
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
