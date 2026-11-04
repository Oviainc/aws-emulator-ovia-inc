// SPDX-License-Identifier: CC0-1.0

//! Pinpoint SMS Voice and Pinpoint SMS Voice V2 share one SigV4 signing name — each must
//! still be claimed by exactly one service in the assembled router.
//!
//! Both models declare `aws.auth#sigv4` name `sms-voice` (V1's endpoint prefix is
//! `sms-voice.pinpoint`, V2's is `sms-voice`, and a host-addressed request resolves to the
//! FIRST label of the endpoint host, which is `sms-voice` either way), and the router
//! resolves a request's routing key from the SigV4 credential scope first, so both see the
//! same key. A service that claims on the signing name alone therefore claims its peer's
//! traffic too, the router finds two claimants and answers `500 AmbiguousService` — which
//! made BOTH services unreachable, every Pinpoint SMS Voice call answering a retryable
//! server fault whose error code is in neither model.
//!
//! Their wire surfaces are disjoint, so each crate's `can_handle` narrows the claim to its
//! own through `service_common::claims_request`: the `restJson1` V1 service claims the
//! `/v1/...` URI path space its operations are bound to (its generated
//! `ROUTE_PATH_SEGMENTS`), and the `awsJson1_0` V2 service claims a request that NAMES it
//! in `X-Amz-Target: PinpointSMSVoiceV2.<Operation>` and arrives on the root path that
//! protocol posts to. This suite pins that split in the production router, across both
//! protocol envelopes, both routing tiers (credential scope and `X-Amz-Target`) and either
//! endpoint host.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `sms-voice` signing name, which is what both
/// services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/sms-voice/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// V1's endpoint host (`awsService.endpointPrefix` `sms-voice.pinpoint`).
const V1_HOST: &str = "sms-voice.pinpoint.us-east-1.amazonaws.com";
/// V2's endpoint host (`awsService.endpointPrefix` `sms-voice`).
const V2_HOST: &str = "sms-voice.us-east-1.amazonaws.com";
/// A loopback endpoint, which names no service at all: the claim must not depend on it.
const LOOPBACK_HOST: &str = "127.0.0.1:9297";

/// What must happen to a request both services could claim on their signing name.
enum Expect {
    /// Served by the service the case names, with this status, and a body containing this
    /// marker. The marker is a string ONLY that service can produce — an output member or
    /// a sentence the other service's model does not carry — so it identifies which crate
    /// answered.
    Serves {
        who: &'static str,
        status: u16,
        marker: &'static str,
    },
    /// Claimed by exactly one of the two: neither `AmbiguousService` (both claimed) nor
    /// `UnknownService` (neither did). All that a request to a still-scaffolded operation
    /// guarantees — such an operation answers the scaffold's `501 NotImplemented`, which
    /// is its own issue, not a routing failure.
    ClaimedByExactlyOne,
}

/// One routing case: the request, and what the router must do with it.
struct Case {
    name: &'static str,
    host: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    /// `X-Amz-Target`, for the awsJson1_0 V2 envelope.
    target: Option<&'static str>,
    /// Whether to sign for `sms-voice`. A case with `false` carries no credential scope,
    /// so the router falls through to the `X-Amz-Target` and then the `Host` routing tier.
    signed: bool,
    body: &'static str,
    expect: Expect,
}

/// The rows differ along what the rule generalises over: which service owns the surface,
/// the protocol envelope (restJson1 method+URI vs awsJson1_0 `POST /` with a target), the
/// routing tier the key comes from (credential scope, `Host`), the endpoint host, and
/// whether the request is a read, a write or a refusal.
///
/// One tier is deliberately not among the rows: an `awsJson` request carrying NO credential
/// scope resolves its routing key from the `X-Amz-Target` service prefix, which the
/// framework reads as the service's SHAPE name (`pinpointsmsvoicev2`) rather than its
/// signing name — a framework-wide tier that has nothing to do with the collision this
/// suite is about, and that no signed SDK call reaches.
const CASES: &[Case] = &[
    Case {
        // V1's own listing, on V1's endpoint host. This is the minimal repro the issue
        // reports as 500 `AmbiguousService`.
        name: "V1 ListConfigurationSets on the V1 host",
        host: V1_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 200,
            marker: "ConfigurationSets",
        },
    },
    Case {
        // The same read on V2's endpoint host: the host does not decide, the modelled wire
        // surface does.
        name: "V1 ListConfigurationSets on the V2 host",
        host: V2_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 200,
            marker: "ConfigurationSets",
        },
    },
    Case {
        // …and on a loopback endpoint, for the same reason.
        name: "V1 ListConfigurationSets on a loopback endpoint",
        host: LOOPBACK_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 200,
            marker: "ConfigurationSets",
        },
    },
    Case {
        // An UNSIGNED request: with no credential scope and no target, the router's last
        // tier is the `Host` label, which is `sms-voice` for V1's endpoint too — so the
        // claim must still split the same way.
        name: "V1 ListConfigurationSets routed by Host alone",
        host: V1_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets",
        target: None,
        signed: false,
        body: "",
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 200,
            marker: "ConfigurationSets",
        },
    },
    Case {
        // A V1 MUTATION: dispatch must reach the handler that decides it, not only the
        // readers. That decision is this account's authorization refusal — a 403 whose
        // message is V1's own, so it shows which service answered just as a served body
        // would.
        name: "V1 CreateConfigurationSet",
        host: V1_HOST,
        method: "POST",
        path_and_query: "/v1/sms-voice/configuration-sets",
        target: None,
        signed: true,
        body: r#"{"ConfigurationSetName":"router-routing-check"}"#,
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 403,
            marker: "Access Denied",
        },
    },
    Case {
        // A V1 REFUSAL path, which the issue reports as 500 `AmbiguousService` too: an
        // empty `@required` path label. The sentence is V1's own, so it also shows which
        // service answered.
        name: "V1 GetConfigurationSetEventDestinations with an empty path label",
        host: V1_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets//event-destinations",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 400,
            marker: "Missing required request parameters: [ConfigurationSetName]",
        },
    },
    Case {
        // The other V1 refusal class, on a query parameter.
        name: "V1 ListConfigurationSets with a garbage NextToken",
        host: V1_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets?NextToken=not-a-token",
        target: None,
        signed: true,
        body: "",
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V1",
            status: 400,
            marker: "The following parameter is null, empty or invalid: ['nextToken']",
        },
    },
    Case {
        // A V1 path carrying an `X-Amz-Target` that names the V2 service: the request
        // contradicts itself, and V1's path space is still its own, so exactly one service
        // claims it rather than both.
        name: "V1 path carrying a V2 X-Amz-Target",
        host: LOOPBACK_HOST,
        method: "GET",
        path_and_query: "/v1/sms-voice/configuration-sets",
        target: Some("PinpointSMSVoiceV2.DescribeConfigurationSets"),
        signed: true,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // V2's own `POST /` traffic, on V2's endpoint host: `ConfigurationSetArn` is an
        // output member V1's model does not declare, so only V2 can have answered.
        name: "V2 CreateConfigurationSet on the V2 host",
        host: V2_HOST,
        method: "POST",
        path_and_query: "/",
        target: Some("PinpointSMSVoiceV2.CreateConfigurationSet"),
        signed: true,
        body: r#"{"ConfigurationSetName":"router-routing-check-v2"}"#,
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V2",
            status: 200,
            marker: "ConfigurationSetArn",
        },
    },
    Case {
        // The same V2 write on V1's endpoint host: again the wire surface decides, not the
        // host.
        name: "V2 CreateConfigurationSet on the V1 host",
        host: V1_HOST,
        method: "POST",
        path_and_query: "/",
        target: Some("PinpointSMSVoiceV2.CreateConfigurationSet"),
        signed: true,
        body: r#"{"ConfigurationSetName":"router-routing-check-v2-again"}"#,
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V2",
            status: 200,
            marker: "ConfigurationSetArn",
        },
    },
    Case {
        // A V2 request on V2's host carrying its target and no body members beyond the
        // required one: still V2's, so the claim does not depend on the payload either.
        name: "V2 CreateConfigurationSet with tags",
        host: V2_HOST,
        method: "POST",
        path_and_query: "/",
        target: Some("PinpointSMSVoiceV2.CreateConfigurationSet"),
        signed: true,
        body: r#"{"ConfigurationSetName":"router-routing-check-v2-tagged",
            "Tags":[{"Key":"owner","Value":"routing-check"}]}"#,
        expect: Expect::Serves {
            who: "Pinpoint SMS Voice V2",
            status: 200,
            marker: "ConfigurationSetArn",
        },
    },
    Case {
        // A V2 operation that is still a scaffold: all that is pinned here is that ONE
        // service claimed it, so its answer is its own 501 rather than a routing failure.
        name: "V2 DescribeConfigurationSets, still scaffolded",
        host: V2_HOST,
        method: "POST",
        path_and_query: "/",
        target: Some("PinpointSMSVoiceV2.DescribeConfigurationSets"),
        signed: true,
        body: "{}",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // A V2 target naming an operation neither model declares: claimed by V2 (it names
        // V2 and arrives on the root), whose own routing failure is the answer — not the
        // router's refusal.
        name: "V2 target naming an operation the model does not declare",
        host: V2_HOST,
        method: "POST",
        path_and_query: "/",
        target: Some("PinpointSMSVoiceV2.NoSuchOperation"),
        signed: true,
        body: "{}",
        expect: Expect::ClaimedByExactlyOne,
    },
];

#[test]
fn pinpoint_sms_voice_v1_and_v2_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("pinpoint-sms-voice-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these cases are about which service claims a request.
                router.set_request_authentication(false);

                for case in CASES {
                    let mut builder = http::Request::builder()
                        .method(case.method)
                        .uri(format!("https://{}{}", case.host, case.path_and_query))
                        .header("host", case.host)
                        .header("content-type", "application/x-amz-json-1.0");
                    if case.signed {
                        builder = builder.header("authorization", AUTHORIZATION);
                    }
                    if let Some(target) = case.target {
                        builder = builder.header("x-amz-target", target);
                    }
                    let request = builder
                        .body(bytes::Bytes::from_static(case.body.as_bytes()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    match &case.expect {
                        Expect::Serves {
                            who,
                            status: expected_status,
                            marker,
                        } => {
                            assert_eq!(
                                status, *expected_status,
                                "case {:?}: expected HTTP {expected_status} from {who}, got \
                                 {status} {body}",
                                case.name
                            );
                            assert!(
                                body.contains(*marker),
                                "case {:?}: expected a body from {who} carrying {marker:?}, \
                                 got {status} {body}",
                                case.name
                            );
                        }
                        Expect::ClaimedByExactlyOne => {}
                    }
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
