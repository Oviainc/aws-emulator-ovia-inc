// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `AssociateExternalConnection`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. Associating a supported public registry stores it on the repository as its ONE
//!    external connection, with the `packageFormat` of that registry and status
//!    `Available`, and answers with the repository record as it now stands — which a
//!    later `DescribeRepository` reports byte-identically.
//! 2. A repository holds at most one external connection: a second, different one is
//!    `ServiceQuotaExceededException` (402) and re-associating the one already held is
//!    `ConflictException` (409); neither writes anything.
//! 3. A connection name that is not a registry CodeArtifact supports is
//!    `ValidationException` (400) and stores nothing.
//! 4. An unresolvable domain or repository (including a `domainOwner` that does not own
//!    the domain) is `ResourceNotFoundException` (404).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header in the shape the router's routing key reader parses:
/// its credential scope is what names the target service AND the region every minted ARN
/// carries. The signature itself is never checked here (the harness turns request
/// authentication off), so it is a placeholder.
const AUTH: &str =
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/codeartifact/aws4_request, \
     SignedHeaders=host, Signature=0";

/// Every external connection CodeArtifact supports, paired with the `packageFormat` its
/// packages carry.
///
/// Source: the `externalConnection` member doc of `AssociateExternalConnectionRequest`
/// (`smithy_query(query="doc",shape="AssociateExternalConnectionRequest",member="externalConnection")`):
/// "The following values are supported: public:npmjs - for the npm public repository.
/// public:nuget-org - for the NuGet Gallery. public:pypi - for the Python Package Index.
/// public:maven-central - for Maven Central. public:maven-googleandroid …
/// public:maven-gradleplugins … public:maven-commonsware … public:maven-clojars …
/// public:ruby-gems-org - for RubyGems.org. public:crates-io - for Crates.io." Each
/// format is a value of the modelled `PackageFormat` enum. Written out here rather than
/// imported from the crate so the catalog is asserted against the model's doc, not
/// against itself.
const SUPPORTED_CONNECTIONS: &[(&str, &str)] = &[
    ("public:npmjs", "npm"),
    ("public:nuget-org", "nuget"),
    ("public:pypi", "pypi"),
    ("public:maven-central", "maven"),
    ("public:maven-googleandroid", "maven"),
    ("public:maven-gradleplugins", "maven"),
    ("public:maven-commonsware", "maven"),
    ("public:maven-clojars", "maven"),
    ("public:ruby-gems-org", "ruby"),
    ("public:crates-io", "cargo"),
];

/// The only value `ExternalConnectionStatus` models.
const AVAILABLE: &str = "Available";

/// One HTTP answer, reduced to what a client branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// `x-amzn-errortype`, which is how restJson1 names the error shape.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// The `x-amzn-errortype` error code, or `""` on a success.
    fn code(&self) -> String {
        self.error_type.clone().unwrap_or_default()
    }

    /// The `repository` member of a success body.
    fn repository(&self) -> &Value {
        &self.body["repository"]
    }
}

/// The emulator under test: the production router with this one service mounted on a
/// single shared engine, exactly as `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Send one request and reduce the response to an [`Answer`].
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "codeartifact.us-east-1.amazonaws.com")
        .header("authorization", AUTH)
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

async fn create_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/domain?domain={domain}"),
        Some(json!({})),
    )
    .await
}

async fn create_repository(router: &MultiServiceRouter, domain: &str, repository: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        Some(json!({ "description": "external connection suite" })),
    )
    .await
}

async fn describe_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/repository?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

async fn associate(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    connection: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!(
            "/v1/repository/external-connection?domain={domain}&repository={repository}\
             &external-connection={}",
            urlencode(connection)
        ),
        Some(json!({})),
    )
    .await
}

/// Percent-encode the characters a connection name may carry that a query string reads
/// specially. `ExternalConnectionName`'s pattern allows `:`, `.`, `-` and `_` only, so
/// `:` is the single character needing it.
fn urlencode(value: &str) -> String {
    value.replace(':', "%3A")
}

/// The `externalConnections` array of a repository description, as the wire reports it.
fn connections(repository: &Value) -> Vec<Value> {
    repository
        .get("externalConnections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Rule 1: every supported registry can be associated, is stored with that registry's
/// package format and the `Available` status, and is reported by a later read.
#[tokio::test]
async fn each_supported_connection_is_stored_with_its_package_format() {
    let router = emulator();
    let domain = "ec-formats-domain";
    assert_eq!(
        create_domain(&router, domain).await.status,
        200,
        "domain setup must succeed"
    );

    for (index, (connection, format)) in SUPPORTED_CONNECTIONS.iter().enumerate() {
        // One repository per case: the one-connection-per-repository rule is rule 2's
        // subject, so each row here starts from a repository with none.
        let repository = format!("ec-repo-{index}");
        let created = create_repository(&router, domain, &repository).await;
        assert_eq!(
            created.status, 200,
            "case {connection:?}: repository setup must succeed, got {created:?}"
        );
        assert!(
            connections(created.repository()).is_empty(),
            "case {connection:?}: a new repository has no external connection, got {:?}",
            connections(created.repository())
        );

        let answer = associate(&router, domain, &repository, connection).await;
        assert_eq!(
            answer.status, 200,
            "case {connection:?}: expected HTTP 200, got {answer:?}"
        );
        let stored = connections(answer.repository());
        assert_eq!(
            stored,
            vec![json!({
                "externalConnectionName": connection,
                "packageFormat": format,
                "status": AVAILABLE,
            })],
            "case {connection:?}: expected one connection with packageFormat {format:?} and status {AVAILABLE:?}, got {stored:?}"
        );

        // Round trip: the read answers with the record the write stored, whole.
        let read = describe_repository(&router, domain, &repository).await;
        assert_eq!(
            read.status, 200,
            "case {connection:?}: read-back must succeed, got {read:?}"
        );
        assert_eq!(
            read.repository(),
            answer.repository(),
            "case {connection:?}: DescribeRepository must report the record the associate stored, got {:?}",
            read.repository()
        );
        // The associate changes ONLY the external connections: everything else the
        // create reported is still what it was.
        for member in ["name", "arn", "domainName", "domainOwner", "administratorAccount", "description", "upstreams", "createdTime"] {
            assert_eq!(
                read.repository().get(member),
                created.repository().get(member),
                "case {connection:?}: {member} must be untouched by the associate, got {:?}",
                read.repository().get(member)
            );
        }
    }
}

/// Rule 2: one external connection per repository — a second, different one is refused
/// as a quota, the one already held is refused as a conflict, and neither writes.
#[tokio::test]
async fn a_repository_holds_at_most_one_external_connection() {
    let router = emulator();
    let domain = "ec-limit-domain";
    assert_eq!(create_domain(&router, domain).await.status, 200);

    // Rows differ in WHICH registry is held first and which second, so the rule is not
    // asserted on one pair: a format-sharing pair (two Maven registries), a
    // format-differing pair, and the repeat of the held one.
    let cases: &[(&str, &str, &str, u16, &str)] = &[
        (
            "second-other-format",
            "public:npmjs",
            "public:pypi",
            402,
            "ServiceQuotaExceededException",
        ),
        (
            "second-same-format",
            "public:maven-central",
            "public:maven-clojars",
            402,
            "ServiceQuotaExceededException",
        ),
        (
            "repeat-of-the-held-one",
            "public:crates-io",
            "public:crates-io",
            409,
            "ConflictException",
        ),
    ];

    for (label, first, second, status, code) in cases {
        let repository = format!("ec-limit-{label}");
        assert_eq!(
            create_repository(&router, domain, &repository).await.status,
            200,
            "case {label:?}: repository setup must succeed"
        );
        let held = associate(&router, domain, &repository, first).await;
        assert_eq!(
            held.status, 200,
            "case {label:?}: the first associate must succeed, got {held:?}"
        );

        let refused = associate(&router, domain, &repository, second).await;
        assert_eq!(
            refused.status, *status,
            "case {label:?}: expected HTTP {status}, got {refused:?}"
        );
        assert_eq!(
            refused.code(),
            *code,
            "case {label:?}: expected {code}, got {:?}",
            refused.code()
        );

        // Nothing was written: the repository still holds exactly the first connection.
        let read = describe_repository(&router, domain, &repository).await;
        assert_eq!(
            read.repository(),
            held.repository(),
            "case {label:?}: a refused associate must not change the record, got {:?}",
            read.repository()
        );
    }
}

/// Rule 3: a name that is not a registry CodeArtifact supports is a `ValidationException`
/// and stores nothing.
#[tokio::test]
async fn an_unsupported_connection_name_is_refused() {
    let router = emulator();
    let domain = "ec-invalid-domain";
    assert_eq!(create_domain(&router, domain).await.status, 200);

    // Each row is a name that satisfies `ExternalConnectionName`'s modelled length and
    // pattern — so the boundary validator admits it — but names no supported registry:
    // an unknown registry, a bare registry name without the `public:` scope, a Maven
    // prefix that is not one of the published Maven connections, and a case variant.
    let cases = [
        "public:not-a-registry",
        "npmjs",
        "public:maven",
        "PUBLIC:NPMJS",
        "public:npmjs.org",
    ];

    for (index, connection) in cases.iter().enumerate() {
        let repository = format!("ec-invalid-{index}");
        assert_eq!(
            create_repository(&router, domain, &repository).await.status,
            200,
            "case {connection:?}: repository setup must succeed"
        );

        let answer = associate(&router, domain, &repository, connection).await;
        assert_eq!(
            answer.status, 400,
            "case {connection:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {connection:?}: expected ValidationException, got {:?}",
            answer.code()
        );

        let read = describe_repository(&router, domain, &repository).await;
        assert!(
            connections(read.repository()).is_empty(),
            "case {connection:?}: a refused associate must store nothing, got {:?}",
            connections(read.repository())
        );
    }
}

/// Rule 4: a domain or repository that does not resolve for this caller is
/// `ResourceNotFoundException` (404).
#[tokio::test]
async fn an_unresolvable_target_is_not_found() {
    let router = emulator();
    let domain = "ec-missing-domain";
    assert_eq!(create_domain(&router, domain).await.status, 200);
    assert_eq!(
        create_repository(&router, domain, "ec-present").await.status,
        200
    );

    // Rows differ in WHAT does not resolve: the domain, the repository inside a domain
    // that does exist, and a `domainOwner` that does not own the domain.
    let cases: &[(&str, String)] = &[
        (
            "unknown-domain",
            format!(
                "/v1/repository/external-connection?domain=ec-absent-domain&repository=ec-present&external-connection={}",
                urlencode("public:npmjs")
            ),
        ),
        (
            "unknown-repository",
            format!(
                "/v1/repository/external-connection?domain={domain}&repository=ec-absent&external-connection={}",
                urlencode("public:npmjs")
            ),
        ),
        (
            "wrong-domain-owner",
            format!(
                "/v1/repository/external-connection?domain={domain}&repository=ec-present&domain-owner=123456789012&external-connection={}",
                urlencode("public:npmjs")
            ),
        ),
    ];

    for (label, uri) in cases {
        let answer = call(&router, "POST", uri, Some(json!({}))).await;
        assert_eq!(
            answer.status, 404,
            "case {label:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            answer.code()
        );
    }
}
