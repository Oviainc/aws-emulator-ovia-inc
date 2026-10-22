// SPDX-License-Identifier: CC0-1.0

//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are THREE emulated services behind
//! ONE SigV4 signing name.
//!
//! All three are generated from the same `AmazonRDSv19` model, whose service traits give
//! each of them `sigv4Name: rds` and `endpointPrefix: rds`, so the default signing-name
//! `can_handle` makes all three claim every `rds`-signed request: the router finds three
//! claimants and answers `500 AmbiguousService` — a code that exists in no AWS model —
//! for ALL of it, leaving every RDS operation unreachable. Worse for an awsQuery client,
//! that router-level refusal is a JSON `__type` body, which botocore's query parser
//! cannot read at all: the caller sees a parse failure instead of an error code.
//!
//! The rule this suite pins: a signing name shared by several emulated services resolves
//! to EXACTLY ONE of them — the service whose own identity is that name, here Amazon RDS
//! — each peer is reachable under the routing key that names IT (`docdb`, `neptune`),
//! and whatever a request gets back, it is that service's own awsQuery XML answer and
//! never a router-level refusal.
//!
//! This suite needs the WHOLE router: the ambiguity exists only when all three services
//! are mounted, so a single-service test cannot see it. Assembling the router costs stack
//! (see `boot_serve.rs`), so it is built — and the requests driven — on an explicitly
//! sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The awsQuery API version all three services share.
const API_VERSION: &str = "2014-10-31";

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// How a request names the service it is for.
enum Addressed {
    /// A SigV4 credential scope naming this service — the signal the router reads first,
    /// and what every AWS SDK sends.
    Signed(&'static str),
    /// A host-style endpoint and no credentials at all: the last routing signal, which
    /// an anonymous request still carries.
    Host(&'static str),
}

/// One call: how it is addressed, the awsQuery `Action` it names, and what the ONE
/// service that owns it must answer.
struct Call {
    case: &'static str,
    addressed: Addressed,
    action: &'static str,
    /// The HTTP status the owning service answers, or `None` when this row only pins
    /// that the request reached exactly one service.
    expected_status: Option<u16>,
    /// The awsQuery `<Code>` the owning service answers with, when the row pins one.
    expected_code: Option<&'static str>,
}

#[test]
fn rds_docdb_and_neptune_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("rds-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The rows differ along BOTH axes that decide where a request lands: which
            // service it names (the shared `rds` name, each peer's own key, a host-style
            // endpoint instead of a credential scope) and which operation it asks for.
            //
            // `DescribeDBClusters` is the discriminator that cannot be faked: Amazon RDS
            // still has it scaffolded (501 `NotImplemented`) while DocumentDB and Neptune
            // implement it (200), so the same Action under three keys must give three
            // different answers — which only happens if each request reached exactly one,
            // correct service. `DescribeDBEngineVersions` is the other way round: RDS
            // implements it and DocumentDB does not.
            let calls: [Call; 7] = [
                Call {
                    // The request the issue reports as `500 AmbiguousService`.
                    case: "rds DescribeDBEngineVersions",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBEngineVersions",
                    expected_status: Some(200),
                    expected_code: None,
                },
                Call {
                    case: "rds DescribeDBInstances",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBInstances",
                    expected_status: Some(200),
                    expected_code: None,
                },
                Call {
                    // Amazon RDS owns the shared name, so its OWN scaffold answers here
                    // — not DocumentDB's or Neptune's implementation.
                    case: "rds DescribeDBClusters",
                    addressed: Addressed::Signed("rds"),
                    action: "DescribeDBClusters",
                    expected_status: Some(501),
                    expected_code: Some("NotImplemented"),
                },
                Call {
                    case: "docdb DescribeDBClusters",
                    addressed: Addressed::Signed("docdb"),
                    action: "DescribeDBClusters",
                    expected_status: Some(200),
                    expected_code: None,
                },
                Call {
                    case: "neptune DescribeDBClusters",
                    addressed: Addressed::Signed("neptune"),
                    action: "DescribeDBClusters",
                    expected_status: Some(200),
                    expected_code: None,
                },
                Call {
                    // DocumentDB's own scaffold, which proves this request did NOT land
                    // on Amazon RDS (where the operation is implemented).
                    case: "docdb DescribeDBEngineVersions",
                    addressed: Addressed::Signed("docdb"),
                    action: "DescribeDBEngineVersions",
                    expected_status: Some(501),
                    expected_code: Some("NotImplemented"),
                },
                Call {
                    // An anonymous request addressed at the RDS endpoint host: the
                    // routing key comes from the host label instead of a credential
                    // scope, and must still resolve to the one service that owns it.
                    case: "rds DescribeDBEngineVersions addressed by host",
                    addressed: Addressed::Host("rds.us-east-1.amazonaws.com"),
                    action: "DescribeDBEngineVersions",
                    expected_status: Some(200),
                    expected_code: None,
                },
            ];

            for call in calls {
                let body = format!("Action={}&Version={API_VERSION}", call.action);
                let mut builder = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("content-type", "application/x-www-form-urlencoded");
                builder = match call.addressed {
                    Addressed::Signed(signing_name) => builder
                        .header("host", "localhost")
                        .header(
                            "authorization",
                            format!(
                                "AWS4-HMAC-SHA256 \
                                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                                 SignedHeaders=host, Signature=not-verified"
                            ),
                        ),
                    Addressed::Host(host) => builder.header("host", host),
                };
                let request = builder
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let case = call.case;

                // Neither routing failure may happen: `AmbiguousService` (500, several
                // claimants) nor `UnknownService` (404, none).
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case:?}: the request must reach exactly one service, got \
                     {status} with {rendered}"
                );
                // Whatever the answer is, it is the target service's own awsQuery
                // envelope — XML, which a query-protocol client can parse — and never
                // the router's JSON `__type` document.
                assert!(
                    rendered.trim_start().starts_with('<') && !rendered.contains("__type"),
                    "case {case:?}: an awsQuery client must get XML, got {status} with \
                     {rendered}"
                );
                if let Some(expected) = call.expected_status {
                    assert_eq!(
                        status, expected,
                        "case {case:?}: the owning service must answer HTTP {expected}, \
                         got {status} with {rendered}"
                    );
                }
                if let Some(expected) = call.expected_code {
                    let code = rendered
                        .split_once("<Code>")
                        .and_then(|(_, rest)| rest.split_once("</Code>"))
                        .map(|(code, _)| code.to_string());
                    assert_eq!(
                        code.as_deref(),
                        Some(expected),
                        "case {case:?}: expected the error code {expected}, got {status} \
                         with {rendered}"
                    );
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("RDS, DocumentDB and Neptune must route apart");
}
