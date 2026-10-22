// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for `DisassociateExternalConnection` — the removal side of
//! the repository external connection that `external_connection.rs` associates.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, using the `pub(crate)` harness
//! `domain_repository_lifecycle.rs` exposes (`emulator`, `call`, `create_domain`,
//! `create_repository`, `describe_repository`, `Answer`) rather than a local copy of it,
//! so every suite in this crate drives the emulator the same way. What is asserted is the
//! wire behaviour a CodeArtifact client sees — status, error code, message and body — not
//! a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. `DisassociateExternalConnection` removes from the addressed repository the ONE
//!    association whose `externalConnectionName` equals the requested name — for every
//!    external connection the service supports — leaves every other member of the record
//!    and every other repository's associations exactly as they were, and answers with the
//!    repository description as it stands AFTER the removal: the same description a later
//!    `DescribeRepository` answers with.
//! 2. A removal target the store does not hold — a domain not held (or held by an account
//!    other than the one the caller pinned), a repository the domain does not hold, or an
//!    external connection the repository is not associated with — is
//!    `ResourceNotFoundException` (404), and nothing is written.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::domain_repository_lifecycle::{
    call, create_domain, create_repository, describe_repository, emulator, Answer, ACCOUNT,
};

/// Every external connection CodeArtifact supports, paired with the `packageFormat` its
/// packages carry.
///
/// Source: the `externalConnection` member doc of `AssociateExternalConnectionRequest`
/// (`smithy_query(query="doc",shape="AssociateExternalConnectionRequest",member="externalConnection")`):
/// "The following values are supported: public:npmjs - for the npm public repository.
/// public:nuget-org - for the NuGet Gallery. public:pypi - for the Python Package Index.
/// public:maven-central - for Maven Central. public:maven-googleandroid …
/// public:maven-gradleplugins … public:maven-commonsware … public:maven-clojars …
/// public:ruby-gems-org - for RubyGems.org. public:crates-io - for Crates.io." Each format
/// is a value of the modelled `PackageFormat` enum. Written out here rather than imported
/// from the crate so the removal is exercised over the model's catalog, not over the
/// crate's copy of it.
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

/// The only value `ExternalConnectionStatus` models
/// (`smithy_query(query="shape",shape="ExternalConnectionStatus")` → `["Available"]`), so
/// it is the status every association reports.
const AVAILABLE: &str = "Available";

/// Percent-encode the one character a connection name may carry that a query string reads
/// specially. `ExternalConnectionName`'s pattern allows `:`, `.`, `-` and alphanumerics.
fn urlencode(value: &str) -> String {
    value.replace(':', "%3A")
}

/// `POST /v1/repository/external-connection` — the association this suite removes.
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

/// `DELETE /v1/repository/external-connection` — the operation under test.
async fn disassociate(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    connection: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/v1/repository/external-connection?domain={domain}&repository={repository}\
             &external-connection={}",
            urlencode(connection)
        ),
        None,
    )
    .await
}

/// The `repository` member of a body.
fn repository_of(answer: &Answer) -> &Value {
    &answer.body["repository"]
}

/// The error body's `message`, or `""` when there is none.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `externalConnections` array of a repository description, as the wire reports it.
fn connections(repository: &Value) -> Vec<Value> {
    repository
        .get("externalConnections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one-entry `externalConnections` array a repository holding `connection` reports.
fn one_connection(connection: &str, format: &str) -> Vec<Value> {
    vec![json!({
        "externalConnectionName": connection,
        "packageFormat": format,
        "status": AVAILABLE,
    })]
}

/// Rule 1: every supported connection can be removed again, the removal is reported and
/// persisted, and it touches neither the rest of the record nor another repository.
#[tokio::test]
async fn every_supported_connection_can_be_disassociated() {
    let router = emulator();
    let domain = "ec-remove-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "domain setup must succeed"
    );

    // One row per supported external connection: the rows differ along what the rule
    // generalises over — the connection name and the package format it carries (several
    // names share `maven`, so the removal is not keyed to the format). The recorded case
    // the issue was filed from, `public:npmjs`, is one row.
    for (index, (connection, format)) in SUPPORTED_CONNECTIONS.iter().enumerate() {
        let repository = format!("ec-remove-{index}");
        // A second repository associated with a DIFFERENT connection, so a removal from
        // one repository is shown not to touch another's associations.
        let sibling = format!("ec-keep-{index}");
        let (sibling_connection, sibling_format) =
            SUPPORTED_CONNECTIONS[(index + 1) % SUPPORTED_CONNECTIONS.len()];

        let created =
            create_repository(&router, domain, &repository, Some("removal suite"), &[]).await;
        assert_eq!(
            created.status, 200,
            "case {connection:?}: repository setup must succeed, got {created:?}"
        );
        assert_eq!(
            create_repository(&router, domain, &sibling, None, &[])
                .await
                .status,
            200,
            "case {connection:?}: sibling setup must succeed"
        );
        let associated = associate(&router, domain, &repository, connection).await;
        assert_eq!(
            associated.status, 200,
            "case {connection:?}: association setup must succeed, got {associated:?}"
        );
        let sibling_associated = associate(&router, domain, &sibling, sibling_connection).await;
        assert_eq!(
            sibling_associated.status, 200,
            "case {connection:?}: sibling association setup must succeed, got {sibling_associated:?}"
        );

        let answer = disassociate(&router, domain, &repository, connection).await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {connection:?}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            connections(repository_of(&answer)),
            Vec::<Value>::new(),
            "case {connection:?}: expected no association left, got {:?}",
            connections(repository_of(&answer))
        );

        // Round trip: the read answers with the record the removal left, whole — so the
        // removal was PERSISTED, not only reported.
        let read = describe_repository(&router, domain, &repository).await;
        assert_eq!(
            read.status, 200,
            "case {connection:?}: read-back must succeed, got {read:?}"
        );
        assert_eq!(
            repository_of(&read),
            repository_of(&answer),
            "case {connection:?}: DescribeRepository must report the record the removal left, got {:?}",
            repository_of(&read)
        );
        // The removal changes ONLY the external connections: every other member the create
        // reported is still what it was.
        for member in [
            "name",
            "arn",
            "domainName",
            "domainOwner",
            "administratorAccount",
            "description",
            "upstreams",
            "createdTime",
        ] {
            assert_eq!(
                repository_of(&read).get(member),
                repository_of(&created).get(member),
                "case {connection:?}: {member} must be untouched by the removal, got {:?}",
                repository_of(&read).get(member)
            );
        }

        let sibling_read = describe_repository(&router, domain, &sibling).await;
        let untouched = one_connection(sibling_connection, sibling_format);
        assert_eq!(
            connections(repository_of(&sibling_read)),
            untouched,
            "case {connection:?}: expected the sibling repository to keep {untouched:?}, got {:?}",
            connections(repository_of(&sibling_read))
        );
        // And the slot the one-connection-per-repository limit allows is free again: the
        // removal freed the repository rather than marking it used up.
        let again = associate(&router, domain, &repository, connection).await;
        assert_eq!(
            (again.status, again.code()),
            (200, String::new()),
            "case {connection:?}: re-associating after the removal must succeed, got {again:?}"
        );
        assert_eq!(
            connections(repository_of(&again)),
            one_connection(connection, format),
            "case {connection:?}: the re-association must be reported, got {:?}",
            connections(repository_of(&again))
        );
    }
}

/// Rule 2: a removal target the store does not hold is `ResourceNotFoundException` (404),
/// and the refusal writes nothing.
#[tokio::test]
async fn a_removal_target_not_held_is_not_found_and_writes_nothing() {
    let router = emulator();
    let domain = "ec-remove-missing-domain";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    for repository in ["connected", "bare"] {
        assert_eq!(
            create_repository(&router, domain, repository, None, &[])
                .await
                .status,
            200,
            "repository {repository:?} setup must succeed"
        );
    }
    let held = associate(&router, domain, "connected", "public:npmjs").await;
    assert_eq!(
        connections(repository_of(&held)),
        one_connection("public:npmjs", "npm"),
        "the association must be in place, got {held:?}"
    );

    // Each row is a removal target the store does not hold, and each expects the SAME
    // answer: 404 `ResourceNotFoundException`. Where the missing target is the association
    // itself, the message is built from the request — it names the external connection
    // asked for and the repository it was asked of.
    let not_associated = |connection: &str, repository: &str| {
        Some(format!(
            "The external connection '{connection}' is not associated with repository '{repository}'"
        ))
    };
    let cases: Vec<(&str, String, Option<String>)> = vec![
        (
            "a repository with no association at all",
            format!(
                "domain={domain}&repository=bare&external-connection={}",
                urlencode("public:npmjs")
            ),
            not_associated("public:npmjs", "bare"),
        ),
        (
            "a connection that is not the one associated",
            format!(
                "domain={domain}&repository=connected&external-connection={}",
                urlencode("public:nuget-org")
            ),
            not_associated("public:nuget-org", "connected"),
        ),
        (
            "a name differing from the associated one only in case",
            format!(
                "domain={domain}&repository=connected&external-connection={}",
                urlencode("PUBLIC:NPMJS")
            ),
            not_associated("PUBLIC:NPMJS", "connected"),
        ),
        (
            "a name that is a prefix of the associated one",
            format!(
                "domain={domain}&repository=connected&external-connection={}",
                urlencode("public:npm")
            ),
            not_associated("public:npm", "connected"),
        ),
        (
            "a repository the domain does not hold",
            format!(
                "domain={domain}&repository=ec-absent&external-connection={}",
                urlencode("public:npmjs")
            ),
            None,
        ),
        (
            "a domain not held",
            format!(
                "domain=ec-absent-domain&repository=connected&external-connection={}",
                urlencode("public:npmjs")
            ),
            None,
        ),
        (
            "a domain pinned to an owner that does not own it",
            format!(
                "domain={domain}&repository=connected&domain-owner=123456789012\
                 &external-connection={}",
                urlencode("public:npmjs")
            ),
            None,
        ),
    ];

    for (label, query, expected_message) in cases {
        let answer = call(
            &router,
            "DELETE",
            &format!("/v1/repository/external-connection?{query}"),
            None,
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: expected 404 ResourceNotFoundException, got {answer:?}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                message(&answer),
                expected,
                "case {label:?}: expected message {expected:?}, got {answer:?}"
            );
        }
        let read = describe_repository(&router, domain, "connected").await;
        assert_eq!(
            repository_of(&read),
            repository_of(&held),
            "case {label:?}: a refused removal must not change the record, got {:?}",
            repository_of(&read)
        );
    }

    // Pinning the owner CORRECTLY resolves: the rule is that the owner must match, not
    // that `domainOwner` must be absent.
    let pinned = call(
        &router,
        "DELETE",
        &format!(
            "/v1/repository/external-connection?domain={domain}&repository=connected\
             &domain-owner={ACCOUNT}&external-connection={}",
            urlencode("public:npmjs")
        ),
        None,
    )
    .await;
    assert_eq!(
        (pinned.status, pinned.code()),
        (200, String::new()),
        "a domainOwner that matches the owner must resolve, got {pinned:?}"
    );
    assert_eq!(
        connections(repository_of(&pinned)),
        Vec::<Value>::new(),
        "the owner-pinned removal must drop the association, got {pinned:?}"
    );
}

/// Rule 2, as idempotence: the removal is observable to the very next call, and it removes
/// the association from the addressed repository only.
#[tokio::test]
async fn a_second_removal_of_the_same_connection_is_refused() {
    let router = emulator();
    let domain = "ec-remove-twice-domain";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    for repository in ["repo", "other"] {
        assert_eq!(
            create_repository(&router, domain, repository, None, &[])
                .await
                .status,
            200,
            "repository {repository:?} setup must succeed"
        );
        let associated = associate(&router, domain, repository, "public:crates-io").await;
        assert_eq!(
            associated.status, 200,
            "repository {repository:?} must be associated, got {associated:?}"
        );
    }

    let first = disassociate(&router, domain, "repo", "public:crates-io").await;
    assert_eq!(
        (first.status, first.code()),
        (200, String::new()),
        "the first removal must succeed, got {first:?}"
    );

    // The association is gone, so the second call addresses a target that is no longer
    // held — the same answer a connection that was never associated gets.
    let second = disassociate(&router, domain, "repo", "public:crates-io").await;
    assert_eq!(
        (second.status, second.code(), message(&second)),
        (
            404,
            "ResourceNotFoundException".to_string(),
            "The external connection 'public:crates-io' is not associated with repository 'repo'"
                .to_string()
        ),
        "the second removal must be refused, got {second:?}"
    );

    // Removing one repository's association is not removing the same name everywhere.
    let other = describe_repository(&router, domain, "other").await;
    assert_eq!(
        connections(repository_of(&other)),
        one_connection("public:crates-io", "cargo"),
        "another repository's association must survive, got {other:?}"
    );
}
