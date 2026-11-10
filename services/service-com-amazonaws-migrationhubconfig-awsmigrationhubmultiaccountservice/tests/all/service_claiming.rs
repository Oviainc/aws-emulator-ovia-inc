// SPDX-License-Identifier: CC0-1.0

//! Which requests this service claims, when another service signs under the same name.
//!
//! MigrationHub Config and Migration Hub are two AWS services behind ONE SigV4 signing
//! name (`mgh`: `smithy_query(query="service_info")` on either model), with distinct
//! endpoint prefixes (`migrationhub-config` and `mgh`) and distinct awsJson1_1
//! `X-Amz-Target` service-shape prefixes (`AWSMigrationHubMultiAccountService` and
//! `AWSMigrationHub`). The signing name therefore identifies neither of them: a request
//! belongs to the service its target prefix names — in any form a client may write it,
//! and whether the request is signed for the shared name or carries no credential scope
//! at all — and a request that names no service, or whose scope names a third one,
//! belongs to nobody here.
//!
//! This suite drives ONLY this service through a router, so the two outcomes are
//! distinguishable: a request this service claims is answered by it, and one it declines
//! leaves the router with no claimant at all (`UnknownService`). It is the mirror of the
//! Migration Hub crate's suite of the same name — the two halves decide one split, so
//! each pins its own side — and
//! `core/router/tests/all/migrationhub_signing_name_routing.rs` pins the rule with both
//! services mounted together.

use std::sync::Arc;

use service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::EmulatedService;

/// This service's own `X-Amz-Target` prefix, on an operation that reads the store.
const CONFIG_TARGET: &str = "AWSMigrationHubMultiAccountService.DescribeHomeRegionControls";

/// The same prefix with the API-version suffix a client may write
/// (`smithy_query(query="service_info")`: version 2019-06-30), and as the fully
/// qualified Coral name — both name this service too.
const CONFIG_TARGET_VERSIONED: &str =
    "AWSMigrationHubMultiAccountService_20190630.DescribeHomeRegionControls";
const CONFIG_TARGET_QUALIFIED: &str = "com.amazonaws.migrationhubconfig.\
AWSMigrationHubMultiAccountService.DescribeHomeRegionControls";

/// The peer service's `X-Amz-Target` prefix, under the same signing name.
const HUB_TARGET: &str = "AWSMigrationHub.ListMigrationTasks";

/// The signing name BOTH services sign with.
const SHARED_SIGNING_NAME: &str = "mgh";

/// A third service's signing name: a request scoped to it is that service's, whatever it
/// targets.
const OTHER_SIGNING_NAME: &str = "s3";

/// What must become of a request: this service answers it, or no service claims it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// Claimed by this service — the answer comes from its model.
    Mine,
    /// Claimed by this service, but the framework's own routing-key reader cannot name an
    /// operation in this target form, so the answer is this service's protocol-correct
    /// routing failure rather than an operation's output. What matters here is that the
    /// claim reached THIS service and not the router's "no claimant" refusal.
    MineUnrouted,
    /// Not this service's request: the router finds no claimant.
    NotMine,
}

/// One row: how the request is addressed, and whether this service claims it.
struct Case {
    name: &'static str,
    host: &'static str,
    target: Option<&'static str>,
    /// The service name in the SigV4 credential scope, or `None` for an unsigned
    /// request, which carries no scope at all.
    scope: Option<&'static str>,
    claim: Claim,
}

/// The rows differ along every signal the rule reads: the target prefix (this service's,
/// in each form a client may write it, the peer's, or none at all), the endpoint host
/// (either service's, or a loopback endpoint that names no service), and the credential
/// scope (the shared signing name, a third service's name, or none).
const CASES: &[Case] = &[
    Case {
        name: "signed, this service's target, its own endpoint host",
        host: "migrationhub-config.us-west-2.amazonaws.com",
        target: Some(CONFIG_TARGET),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::Mine,
    },
    Case {
        name: "signed, this service's target, a loopback endpoint",
        host: "127.0.0.1:9297",
        target: Some(CONFIG_TARGET),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::Mine,
    },
    Case {
        name: "signed, this service's target, the peer's endpoint host",
        host: "mgh.us-west-2.amazonaws.com",
        target: Some(CONFIG_TARGET),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::Mine,
    },
    Case {
        name: "signed, this service's target with its API-version suffix",
        host: "127.0.0.1:9297",
        target: Some(CONFIG_TARGET_VERSIONED),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::Mine,
    },
    Case {
        name: "signed, this service's target as a fully qualified name",
        host: "127.0.0.1:9297",
        target: Some(CONFIG_TARGET_QUALIFIED),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::MineUnrouted,
    },
    Case {
        // No credential scope at all: the router resolves the key from the target
        // prefix instead, and it still names this service.
        name: "unsigned, this service's target",
        host: "127.0.0.1:9297",
        target: Some(CONFIG_TARGET),
        scope: None,
        claim: Claim::Mine,
    },
    Case {
        name: "unsigned, this service's target, its own endpoint host",
        host: "migrationhub-config.us-west-2.amazonaws.com",
        target: Some(CONFIG_TARGET),
        scope: None,
        claim: Claim::Mine,
    },
    Case {
        name: "signed, the peer's target, this service's endpoint host",
        host: "migrationhub-config.us-west-2.amazonaws.com",
        target: Some(HUB_TARGET),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::NotMine,
    },
    Case {
        name: "signed, the peer's target, the peer's endpoint host",
        host: "mgh.us-west-2.amazonaws.com",
        target: Some(HUB_TARGET),
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::NotMine,
    },
    Case {
        name: "unsigned, the peer's target",
        host: "127.0.0.1:9297",
        target: Some(HUB_TARGET),
        scope: None,
        claim: Claim::NotMine,
    },
    Case {
        // Scoped to a third service: that service's request, however it is targeted.
        name: "signed for another service, this service's target",
        host: "127.0.0.1:9297",
        target: Some(CONFIG_TARGET),
        scope: Some(OTHER_SIGNING_NAME),
        claim: Claim::NotMine,
    },
    Case {
        // An awsJson1_1 request with no target names no service and no operation — not
        // even on this service's own endpoint host — so nobody claims it and the router
        // answers its own 404 rather than one service guessing.
        name: "signed, no target, this service's endpoint host",
        host: "migrationhub-config.us-west-2.amazonaws.com",
        target: None,
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::NotMine,
    },
    Case {
        name: "unsigned, no target, this service's endpoint host",
        host: "migrationhub-config.us-west-2.amazonaws.com",
        target: None,
        scope: None,
        claim: Claim::NotMine,
    },
    Case {
        name: "signed, no target, the peer's endpoint host",
        host: "mgh.us-west-2.amazonaws.com",
        target: None,
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::NotMine,
    },
    Case {
        name: "signed, no target, a loopback endpoint",
        host: "127.0.0.1:9297",
        target: None,
        scope: Some(SHARED_SIGNING_NAME),
        claim: Claim::NotMine,
    },
];

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. These
/// cases are about which service claims a request, not about SigV4, so the signature
/// itself is a placeholder and the router's request authentication is off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEMULATOR/20260102/us-west-2/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

#[tokio::test]
async fn the_target_prefix_and_endpoint_host_decide_which_requests_this_service_claims() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    for case in CASES {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!("https://{}/", case.host))
            .header("host", case.host)
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(target) = case.target {
            builder = builder.header("x-amz-target", target);
        }
        if let Some(scope) = case.scope {
            builder = builder.header("authorization", authorization(scope));
        }
        let request = builder
            .body(bytes::Bytes::from_static(b"{}"))
            .expect("a valid request");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

        match (case.claim, case.target) {
            (Claim::Mine, _) => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected HTTP 200 from this service, got {status} {body}",
                    case.name
                );
                assert!(
                    body.contains("HomeRegionControls"),
                    "case {:?}: expected this service's own answer, got {status} {body}",
                    case.name
                );
            }
            (Claim::MineUnrouted, _) => {
                assert!(
                    !body.contains("UnknownService"),
                    "case {:?}: this service must claim the request, got {status} {body}",
                    case.name
                );
                assert_eq!(
                    (status, body.contains("NotImplemented")),
                    (501, true),
                    "case {:?}: expected this service's own routing failure, got {status} \
                     {body}",
                    case.name
                );
            }
            (Claim::NotMine, _) => {
                assert_eq!(
                    (status, body.contains("UnknownService")),
                    (404, true),
                    "case {:?}: this service must not claim the request, got {status} {body}",
                    case.name
                );
            }
        }
    }
}
