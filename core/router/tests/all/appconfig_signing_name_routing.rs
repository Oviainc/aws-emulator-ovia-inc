// SPDX-License-Identifier: CC0-1.0

//! AppConfig and AppConfigData share one SigV4 signing name — each must still be
//! claimed by exactly one service in the assembled router.
//!
//! Both services sign as `appconfig` (that is AppConfigData's `aws.auth#sigv4` name
//! too), so the routing key alone matches both and the router rejects a request two
//! services claim with `500 AmbiguousService` — which would make BOTH services
//! unreachable. Their modelled URI path spaces are disjoint, so each crate's
//! `can_handle` adds the framework's path-segment conjunct (the rule
//! `emulator_server::can_handle_based_on_route_key_and_path` implements) over its own
//! generated `ROUTE_PATH_SEGMENTS`. This suite pins that split in the production
//! router: the endpoint host does not matter, the modelled path space does.
//!
//! The data plane serves a session only over an (application, environment,
//! configuration profile) triple the control plane holds, so the suite creates that
//! hierarchy first — through the control plane in this same router, which is itself part
//! of what is being checked.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for
//! the same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `appconfig` signing name, which is what both
/// services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/appconfig/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request both services could claim on their signing name.
enum Expect {
    /// Served by the AppConfigData data plane: `StartConfigurationSession` is
    /// implemented, so a 201 carrying a session token proves which crate answered
    /// (the control plane binds no `/configurationsessions` path at all).
    DataPlaneServes,
    /// Claimed by the AppConfig control plane: the assertion is that the data plane
    /// did NOT answer it. Deliberately says nothing about the control plane's own
    /// status, which changes as its operations are implemented.
    ControlPlaneClaims,
    /// Claimed by exactly one of the two: neither `AmbiguousService` (both claimed)
    /// nor `UnknownService` (neither did). All a near-miss path guarantees.
    ClaimedByExactlyOne,
}

/// One routing case.
struct Case {
    name: &'static str,
    host: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    body: &'static str,
    expect: Expect,
}

const SESSION_BODY: &str = r#"{"ApplicationIdentifier":"MyApp","EnvironmentIdentifier":"Prod","ConfigurationProfileIdentifier":"MyProfile"}"#;

const CASES: &[Case] = &[
    Case {
        name: "data-plane path on the data-plane endpoint host",
        host: "appconfigdata.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/configurationsessions",
        body: SESSION_BODY,
        expect: Expect::DataPlaneServes,
    },
    Case {
        name: "data-plane path on a loopback endpoint",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/configurationsessions",
        body: SESSION_BODY,
        expect: Expect::DataPlaneServes,
    },
    Case {
        name: "data-plane path even on the control-plane endpoint host",
        host: "appconfig.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/configurationsessions",
        body: SESSION_BODY,
        expect: Expect::DataPlaneServes,
    },
    Case {
        name: "control-plane path on the control-plane endpoint host",
        host: "appconfig.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/applications",
        body: r#"{"Name":"ControlPlaneApp"}"#,
        expect: Expect::ControlPlaneClaims,
    },
    Case {
        name: "control-plane path on a loopback endpoint",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/applications",
        body: r#"{"Name":"OtherControlPlaneApp"}"#,
        expect: Expect::ControlPlaneClaims,
    },
    Case {
        name: "data-plane poll with a query string",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/configuration?configuration_token=not-a-real-token",
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "data-plane path with a trailing slash",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/configurationsessions/",
        body: SESSION_BODY,
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "control-plane path with a trailing slash",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/applications/",
        body: r#"{"Name":"TrailingSlashApp"}"#,
        expect: Expect::ClaimedByExactlyOne,
    },
];

/// The AppConfig control-plane endpoint host, used for the setup calls below.
const CONTROL_PLANE_HOST: &str = "appconfig.us-east-1.amazonaws.com";

/// Send one request through the assembled router and answer its status and body.
async fn send(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path_and_query: &str,
    body: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path_and_query}"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::copy_from_slice(body.as_bytes()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();
    (status, body)
}

/// Create the application, environment and configuration profile `SESSION_BODY` names,
/// through the AppConfig control plane in this same router.
///
/// AppConfigData is AppConfig's DATA plane: a session is started only over a triple the
/// control plane actually holds, so the hierarchy has to exist before the data plane can
/// serve one. Driving the setup through the same router is also part of what this suite
/// checks — both services answer their own paths in one assembled emulator.
async fn create_hierarchy(router: &MultiServiceRouter) {
    let (status, body) = send(
        router,
        CONTROL_PLANE_HOST,
        "POST",
        "/applications",
        r#"{"Name":"MyApp"}"#,
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateApplication: expected 201, got {status} {body}"
    );
    let application_id = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|document| document["Id"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("setup CreateApplication: no Id in {body}"));

    for (path, request_body) in [
        (
            format!("/applications/{application_id}/environments"),
            r#"{"Name":"Prod"}"#,
        ),
        (
            format!("/applications/{application_id}/configurationprofiles"),
            r#"{"Name":"MyProfile","LocationUri":"hosted"}"#,
        ),
    ] {
        let (status, body) =
            send(router, CONTROL_PLANE_HOST, "POST", &path, request_body).await;
        assert_eq!(
            status, 201,
            "setup POST {path}: expected 201, got {status} {body}"
        );
    }
}

#[test]
fn appconfig_and_appconfigdata_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("appconfig-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let router = build_router(engine);
                create_hierarchy(&router).await;

                for case in CASES {
                    let (status, body) = send(
                        &router,
                        case.host,
                        case.method,
                        case.path_and_query,
                        case.body,
                    )
                    .await;

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
                    match case.expect {
                        Expect::DataPlaneServes => {
                            assert_eq!(
                                status, 201,
                                "case {}: the data plane must serve this request, got {status} {body}",
                                case.name
                            );
                            assert!(
                                body.contains("InitialConfigurationToken"),
                                "case {}: expected a session token, got {status} {body}",
                                case.name
                            );
                        }
                        Expect::ControlPlaneClaims => {
                            assert!(
                                !body.contains("InitialConfigurationToken"),
                                "case {}: the data plane answered a control-plane request: {status} {body}",
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
