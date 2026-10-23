// SPDX-License-Identifier: CC0-1.0

//! What this service's DELETEs answer, asserted end-to-end over the wire.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with (the shared `harness` module), so what is pinned here
//! is the status, the error shape and the body a CodeArtifact client reads — not a
//! handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. A delete answers for the POST-delete state of the resource it removed: the
//!    `DomainDescription` `DeleteDomain` hands back is the projection every read of that
//!    domain builds, with `status` moved to the terminal `Deleted` and every other member
//!    byte-identical to the last read before the call.
//! 2. The absence of the resource a delete NAMES is the call's goal, not a precondition:
//!    deleting one this account does not hold answers HTTP 200 carrying no removed
//!    resource — for a policy delete, the bare `ResourcePolicy` envelope naming the
//!    resource the request targeted — with no error shape, and changes nothing.
//! 3. The SCOPE a request is addressed to is a different matter: a domain (and, for a
//!    repository-scoped call, its repository) that this caller does not hold is still the
//!    modelled `ResourceNotFoundException` (404).
//! 4. A precondition is still a refusal: a domain that still holds repositories is
//!    `ConflictException` (409) and survives.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT, REGION};

/// Every value `DomainStatus` models, in model order.
/// Source: `smithy_query(query="shape",shape="DomainStatus")` — enum values
/// `[Active, Deleted]`.
const DOMAIN_STATUS_VALUES: [&str; 2] = ["Active", "Deleted"];

/// A well-formed 12-digit account that is NOT the one this emulator serves (`AccountId` is
/// modelled `len 12..12, pattern ^[0-9]{12}$`), used to address a domain this account does
/// not hold. Asserted distinct from [`ACCOUNT`] where it is used.
const FOREIGN_ACCOUNT: &str = "111122223333";

async fn describe_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await
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

/// `DeleteDomain`: `DELETE /v1/domain?domain=&domain-owner=`
/// (`smithy_query(query="operation",operation="DeleteDomain")`).
async fn delete_domain(
    router: &MultiServiceRouter,
    domain: &str,
    domain_owner: Option<&str>,
) -> Answer {
    let mut uri = format!("/v1/domain?domain={domain}");
    if let Some(owner) = domain_owner {
        uri.push_str(&format!("&domain-owner={owner}"));
    }
    call(router, "DELETE", &uri, None).await
}

/// `DeleteRepository`: `DELETE /v1/repository?domain=&repository=&domain-owner=`.
async fn delete_repository(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    domain_owner: Option<&str>,
) -> Answer {
    let mut uri = format!("/v1/repository?domain={domain}&repository={repository}");
    if let Some(owner) = domain_owner {
        uri.push_str(&format!("&domain-owner={owner}"));
    }
    call(router, "DELETE", &uri, None).await
}

/// `DeleteDomainPermissionsPolicy`: `DELETE /v1/domain/permissions/policy`.
async fn delete_domain_policy(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/domain/permissions/policy?domain={domain}"),
        None,
    )
    .await
}

/// `DeleteRepositoryPermissionsPolicy`: `DELETE /v1/repository/permissions/policies` —
/// the PLURAL path segment the model binds this one operation to.
async fn delete_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/repository/permissions/policies?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

/// `PutDomainPermissionsPolicy`, used to attach a policy a later case removes.
async fn put_domain_policy(router: &MultiServiceRouter, domain: &str, document: &str) -> Answer {
    call(
        router,
        "PUT",
        "/v1/domain/permissions/policy",
        Some(json!({ "domain": domain, "policyDocument": document })),
    )
    .await
}

/// The domain ARN every domain-scoped `resourceArn` is reported under.
fn domain_arn(domain: &str) -> String {
    format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}")
}

/// The repository ARN a repository's policy is reported under.
fn repository_arn(domain: &str, repository: &str) -> String {
    format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/{domain}/{repository}")
}

/// One domain to build up and tear down: the name, how many repositories it holds while
/// alive, and the `encryptionKey` the create pinned (none for an AWS-managed key).
struct DomainCase {
    label: &'static str,
    domain: String,
    repositories: usize,
    encryption_key: Option<String>,
}

/// RULE 1: the description a delete answers with reports the state the resource reached by
/// being deleted — `status` at the terminal `Deleted`, everything else exactly as the last
/// read reported it.
///
/// Rows differ along what the rule generalises over: the domain name (the modelled minimum
/// length 2, a hyphenated name, the modelled maximum length 50), how many repositories the
/// domain held before being emptied (0, 1, 3 — so `repositoryCount` and `assetSizeBytes`
/// differ between rows), and whether the caller pinned an `encryptionKey`.
#[tokio::test]
async fn delete_domain_answers_with_the_terminal_status() {
    let router = emulator();

    let cases = [
        DomainCase {
            label: "shortest name, empty, AWS-managed key",
            domain: "ab".to_string(),
            repositories: 0,
            encryption_key: None,
        },
        DomainCase {
            label: "hyphenated name, one repository, pinned key",
            domain: "delete-status".to_string(),
            repositories: 1,
            encryption_key: Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"
            )),
        },
        DomainCase {
            label: "longest name, three repositories, AWS-managed key",
            domain: format!("d{}e", "x".repeat(48)),
            repositories: 3,
            encryption_key: None,
        },
    ];

    for case in &cases {
        let label = case.label;
        let domain = case.domain.as_str();

        let created = create_domain(&router, domain, case.encryption_key.as_deref()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected CreateDomain 200, got {created:?}"
        );
        for index in 0..case.repositories {
            let repository = format!("Repo-{index}");
            let made = create_repository(&router, domain, &repository, None, &[]).await;
            assert_eq!(
                made.status, 200,
                "case {label:?}: expected CreateRepository 200 for {repository:?}, got {made:?}"
            );
            // Emptied again, because a domain is deletable only once it holds none.
            let removed = delete_repository(&router, domain, &repository, None).await;
            assert_eq!(
                removed.status, 200,
                "case {label:?}: expected DeleteRepository 200 for {repository:?}, got {removed:?}"
            );
        }

        // The last read before the delete: everything the delete reports, bar the status,
        // has to be this record.
        let read = describe_domain(&router, domain).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected DescribeDomain 200, got {read:?}"
        );
        assert_eq!(
            read.body["domain"]["status"].as_str(),
            Some(DOMAIN_STATUS_VALUES[0]),
            "case {label:?}: a held domain reports the live status, got {:?}",
            read.body
        );

        let deleted = delete_domain(&router, domain, None).await;
        let mut expected = read.body["domain"].clone();
        expected["status"] = json!(DOMAIN_STATUS_VALUES[1]);
        assert_eq!(
            (deleted.status, deleted.body["domain"].clone()),
            (200, expected),
            "case {label:?}: the delete must answer with the removed record at the terminal \
             status, got {deleted:?}"
        );

        // ...and the resource really is gone: the next read and the next list say so.
        let gone = describe_domain(&router, domain).await;
        assert_eq!(
            (gone.status, gone.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a deleted domain must be gone from the next read, got {gone:?}"
        );
        let listed = call(&router, "POST", "/v1/domains", Some(json!({}))).await;
        let names: Vec<&str> = listed.body["domains"]
            .as_array()
            .map(|domains| {
                domains
                    .iter()
                    .filter_map(|summary| summary["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !names.contains(&domain),
            "case {label:?}: a deleted domain must be gone from ListDomains, got {names:?}"
        );
    }
}

/// One idempotent-delete row: the request, and the body the rule requires.
struct AbsentCase {
    label: String,
    method: &'static str,
    uri: String,
    /// The whole response body the delete must answer with.
    body: Value,
}

/// RULE 2: a delete whose named target this account does not hold answers 200 and reports
/// no removed resource — never an error shape.
///
/// Rows differ along what the rule generalises over: the resource kind (domain,
/// repository, domain policy, repository policy), WHY the target is absent (never created,
/// already deleted by a previous identical call, held by another account), and the name
/// shapes the request addresses it by, including the modelled length extremes.
#[tokio::test]
async fn deleting_a_target_that_is_not_held_succeeds_and_reports_nothing() {
    let router = emulator();

    let host_domain = "idempotent";
    assert_eq!(
        create_domain(&router, host_domain, None).await.status,
        200,
        "setup: the host domain must be created"
    );

    // A domain, a repository and both policies that DID exist, deleted once here so the
    // table below can delete them a second time.
    let recreated = "deleted-once";
    assert_eq!(
        create_domain(&router, recreated, None).await.status,
        200,
        "setup: the domain deleted twice must be created"
    );
    assert_eq!(
        delete_domain(&router, recreated, None).await.status,
        200,
        "setup: the first delete of that domain must succeed"
    );

    let short_repo = "Rr";
    assert_eq!(
        create_repository(&router, host_domain, short_repo, None, &[])
            .await
            .status,
        200,
        "setup: the repository deleted twice must be created"
    );
    assert_eq!(
        delete_repository(&router, host_domain, short_repo, None)
            .await
            .status,
        200,
        "setup: the first delete of that repository must succeed"
    );

    let policy_document = r#"{"Version":"2012-10-17","Statement":[{"Sid":"S","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;
    let policied = "had-a-policy";
    assert_eq!(
        create_domain(&router, policied, None).await.status,
        200,
        "setup: the domain whose policy is removed must be created"
    );
    assert_eq!(
        put_domain_policy(&router, policied, policy_document)
            .await
            .status,
        200,
        "setup: the policy to remove must be attached"
    );
    assert_eq!(
        delete_domain_policy(&router, policied).await.status,
        200,
        "setup: the first delete of that policy must succeed"
    );

    // A domain this emulator's account does not hold, because the request pins it to
    // another one. It must survive every row below.
    assert_ne!(
        FOREIGN_ACCOUNT, ACCOUNT,
        "the wrong-owner rows need an account the emulator does not present"
    );
    let owned_here = "owned-here";
    assert_eq!(
        create_domain(&router, owned_here, None).await.status,
        200,
        "setup: the wrong-owner domain must be created"
    );
    let owned_here_repo = "Owned";
    assert_eq!(
        create_repository(&router, owned_here, owned_here_repo, None, &[])
            .await
            .status,
        200,
        "setup: the wrong-owner repository must be created"
    );

    // The modelled extremes: `DomainName` is len 2..50 and `RepositoryName` len 2..100.
    let longest_domain = format!("n{}e", "o".repeat(48));
    let longest_repo = format!("N{}9", "o".repeat(98));

    let cases = vec![
        AbsentCase {
            label: "DeleteDomain, never created, 2-character name".to_string(),
            method: "DELETE",
            uri: "/v1/domain?domain=zz".to_string(),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteDomain, never created, hyphenated name".to_string(),
            method: "DELETE",
            uri: "/v1/domain?domain=no-such-domain".to_string(),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteDomain, never created, 50-character name".to_string(),
            method: "DELETE",
            uri: format!("/v1/domain?domain={longest_domain}"),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteDomain, already deleted by the previous call".to_string(),
            method: "DELETE",
            uri: format!("/v1/domain?domain={recreated}"),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteDomain, held by another account".to_string(),
            method: "DELETE",
            uri: format!("/v1/domain?domain={owned_here}&domain-owner={FOREIGN_ACCOUNT}"),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteRepository, never created, 2-character name".to_string(),
            method: "DELETE",
            uri: format!("/v1/repository?domain={host_domain}&repository=Ab"),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteRepository, never created, 100-character name".to_string(),
            method: "DELETE",
            uri: format!("/v1/repository?domain={host_domain}&repository={longest_repo}"),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteRepository, already deleted by the previous call".to_string(),
            method: "DELETE",
            uri: format!("/v1/repository?domain={host_domain}&repository={short_repo}"),
            body: json!({}),
        },
        AbsentCase {
            label: "DeleteDomainPermissionsPolicy, never attached".to_string(),
            method: "DELETE",
            uri: format!("/v1/domain/permissions/policy?domain={host_domain}"),
            body: json!({ "policy": { "resourceArn": domain_arn(host_domain) } }),
        },
        AbsentCase {
            label: "DeleteDomainPermissionsPolicy, already removed".to_string(),
            method: "DELETE",
            uri: format!("/v1/domain/permissions/policy?domain={policied}"),
            body: json!({ "policy": { "resourceArn": domain_arn(policied) } }),
        },
        AbsentCase {
            label: "DeleteRepositoryPermissionsPolicy, never attached".to_string(),
            method: "DELETE",
            uri: format!(
                "/v1/repository/permissions/policies?domain={owned_here}&repository={owned_here_repo}"
            ),
            body: json!({
                "policy": { "resourceArn": repository_arn(owned_here, owned_here_repo) }
            }),
        },
    ];

    for case in &cases {
        let label = &case.label;
        let answer = call(&router, case.method, &case.uri, None).await;
        assert_eq!(
            (answer.status, answer.body.clone()),
            (200, case.body.clone()),
            "case {label:?}: deleting what is not held must succeed with no removed resource, \
             got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            String::new(),
            "case {label:?}: a successful delete carries no error shape, got {answer:?}"
        );
    }

    // ...and none of those calls removed anything: the domain a row addressed through the
    // wrong owner, and its repository, are still readable by the account that owns them.
    let survivor = describe_domain(&router, owned_here).await;
    assert_eq!(
        (survivor.status, survivor.body["domain"]["status"].as_str()),
        (200, Some(DOMAIN_STATUS_VALUES[0])),
        "a delete pinned to another account must not remove the domain, got {survivor:?}"
    );
    let survivor_repo = describe_repository(&router, owned_here, owned_here_repo).await;
    assert_eq!(
        survivor_repo.status, 200,
        "a delete pinned to another account must not remove its repositories, got \
         {survivor_repo:?}"
    );
}

/// RULE 3: the scope a delete is addressed to still has to resolve — only the target's own
/// absence is success.
///
/// Rows differ along the operation and which part of the scope is missing: the domain, the
/// repository a repository-scoped policy call names, and a domain that exists but is
/// addressed through an account that does not own it.
#[tokio::test]
async fn the_scope_a_delete_addresses_must_still_resolve() {
    let router = emulator();

    let held = "scoped";
    assert_eq!(
        create_domain(&router, held, None).await.status,
        200,
        "setup: the held domain must be created"
    );
    let held_repo = "Held";
    assert_eq!(
        create_repository(&router, held, held_repo, None, &[])
            .await
            .status,
        200,
        "setup: the held repository must be created"
    );
    assert_ne!(
        FOREIGN_ACCOUNT, ACCOUNT,
        "the wrong-owner rows need an account the emulator does not present"
    );

    let refusals = vec![
        (
            "DeleteRepository in a domain that is not held",
            delete_repository(&router, "no-such-domain", held_repo, None).await,
            "domain",
        ),
        (
            "DeleteRepository in a domain pinned to another account",
            delete_repository(&router, held, held_repo, Some(FOREIGN_ACCOUNT)).await,
            "domain",
        ),
        (
            "DeleteDomainPermissionsPolicy on a domain that is not held",
            delete_domain_policy(&router, "no-such-domain").await,
            "domain",
        ),
        (
            "DeleteRepositoryPermissionsPolicy in a domain that is not held",
            delete_repository_policy(&router, "no-such-domain", held_repo).await,
            "domain",
        ),
        (
            "DeleteRepositoryPermissionsPolicy for a repository that is not held",
            delete_repository_policy(&router, held, "NoSuchRepo").await,
            "repository",
        ),
    ];

    for (label, answer, resource_type) in refusals {
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: an unresolvable scope must be not found, got {answer:?}"
        );
        assert_eq!(
            answer.body["resourceType"].as_str(),
            Some(resource_type),
            "case {label:?}: the refusal must name the resource kind it could not resolve, \
             got {answer:?}"
        );
    }

    // The scope refusals removed nothing.
    let intact = describe_repository(&router, held, held_repo).await;
    assert_eq!(
        intact.status, 200,
        "the held repository must survive every scope refusal, got {intact:?}"
    );
}

/// RULE 4: idempotence is about the target's ABSENCE, not about refusing nothing — a
/// precondition still refuses, and the resource survives.
///
/// Rows differ along the number of repositories the domain holds.
#[tokio::test]
async fn deleting_a_domain_that_still_holds_repositories_conflicts() {
    let router = emulator();

    for (domain, repositories) in [("one", 1usize), ("two", 2), ("three", 3)] {
        assert_eq!(
            create_domain(&router, domain, None).await.status,
            200,
            "case {domain:?}: the domain must be created"
        );
        for index in 0..repositories {
            assert_eq!(
                create_repository(&router, domain, &format!("Repo-{index}"), None, &[])
                    .await
                    .status,
                200,
                "case {domain:?}: repository {index} must be created"
            );
        }

        let refused = delete_domain(&router, domain, None).await;
        assert_eq!(
            (refused.status, refused.code()),
            (409, "ConflictException".to_string()),
            "case {domain:?}: a domain holding {repositories} repositories must conflict, got \
             {refused:?}"
        );
        let survived = describe_domain(&router, domain).await;
        assert_eq!(
            (
                survived.status,
                survived.body["domain"]["repositoryCount"].as_i64()
            ),
            (200, Some(repositories as i64)),
            "case {domain:?}: a refused delete must change nothing, got {survived:?}"
        );
    }
}
