// SPDX-License-Identifier: CC0-1.0

//! A public repository's two sub-resources as its REPOSITORY-LEVEL readers answer for
//! them, driven end to end over the wire: which repository a policy operation is about,
//! and the catalog data `GetRepositoryCatalogData` reports.
//!
//! The rules under test:
//!
//! * A repository policy is a sub-resource of a repository, so every policy operation
//!   resolves the REPOSITORY first: a name this registry does not hold is 400
//!   `RepositoryNotFoundException`, a `registryId` naming another registry is 400
//!   `InvalidParameterException`, and only a repository that exists and carries no policy
//!   is 400 `RepositoryPolicyNotFoundException`. (What a policy document itself must be,
//!   and the set/read/replace/delete cycle over one, is `tests/all/repository_policy.rs`.)
//! * Catalog data is a sub-resource every repository HAS: a read reports what the create
//!   persisted (the default record for a repository created without any), and 400
//!   `RepositoryNotFoundException` for a repository that does not exist.
//!
//! The tables span what those rules generalise over — every form the modelled
//! `repositoryName` pattern admits, each of the three policy operations, and every member
//! of `RepositoryCatalogDataInput` a create can state — rather than one recorded request.

use serde_json::{json, Value};

use super::harness::error_message;
use crate::repository_image_lifecycle::{ok, refused, service, REGISTRY_ID};

#[tokio::test]
async fn every_policy_operation_refuses_a_repository_the_registry_does_not_hold() {
    let router = service();
    // Every name form the modelled `repositoryName` pattern admits, so the refusal is
    // about the repository being absent and not about the name's shape.
    for repository in ["absent", "team/absent", "a.b_c-d/absent.one"] {
        for (operation, body) in [
            ("GetRepositoryPolicy", json!({"repositoryName": repository})),
            (
                "DeleteRepositoryPolicy",
                json!({"repositoryName": repository}),
            ),
            (
                "SetRepositoryPolicy",
                json!({"repositoryName": repository, "policyText": "{}"}),
            ),
        ] {
            let body = refused(
                &router,
                operation,
                body,
                "RepositoryNotFoundException",
                repository,
            )
            .await;
            let message = error_message(&body);
            assert!(
                message.contains(repository) && message.contains(REGISTRY_ID),
                "case {repository:?}: expected {operation}'s refusal to name the repository \
                 and the registry, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_policy_operation_naming_another_registry_is_refused() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;
    for (operation, body) in [
        (
            "GetRepositoryPolicy",
            json!({"repositoryName": "team/app", "registryId": "123456789012"}),
        ),
        (
            "DeleteRepositoryPolicy",
            json!({"repositoryName": "team/app", "registryId": "123456789012"}),
        ),
        (
            "SetRepositoryPolicy",
            json!({
                "repositoryName": "team/app",
                "registryId": "123456789012",
                "policyText": "{}",
            }),
        ),
    ] {
        refused(
            &router,
            operation,
            body,
            "InvalidParameterException",
            "another registry",
        )
        .await;
    }
    // A repository that exists and carries no policy is the OTHER refusal, so the two are
    // distinguishable — and the refused `Set` above persisted nothing.
    refused(
        &router,
        "GetRepositoryPolicy",
        json!({"repositoryName": "team/app"}),
        "RepositoryPolicyNotFoundException",
        "after a refused set",
    )
    .await;
    refused(
        &router,
        "DeleteRepositoryPolicy",
        json!({"repositoryName": "team/app"}),
        "RepositoryPolicyNotFoundException",
        "after a refused set",
    )
    .await;
}

/// One repository's catalog data as `CreateRepository` states it.
struct CatalogCase {
    case: &'static str,
    repository: &'static str,
    /// The `catalogData` member of the create request, or `None` for a create that states
    /// none at all.
    catalog_data: Option<Value>,
}

/// Catalog data spanning every member `RepositoryCatalogDataInput` declares: none at all,
/// one text member, every text member, the two list members, and a logo payload (whose
/// `logoUrl` the service derives from the bytes).
fn catalog_cases() -> Vec<CatalogCase> {
    vec![
        CatalogCase {
            case: "no catalog data at all",
            repository: "catalog/none",
            catalog_data: None,
        },
        CatalogCase {
            case: "a description only",
            repository: "catalog/description",
            catalog_data: Some(json!({"description": "a sample repository"})),
        },
        CatalogCase {
            case: "every text member",
            repository: "catalog/text",
            catalog_data: Some(json!({
                "description": "all of the text",
                "aboutText": "# About\n\nthis repository",
                "usageText": "docker pull public.ecr.aws/emulator/catalog/text",
            })),
        },
        CatalogCase {
            case: "the list members",
            repository: "catalog/lists",
            catalog_data: Some(json!({
                "architectures": ["ARM 64", "x86-64"],
                "operatingSystems": ["Linux", "Windows"],
            })),
        },
        CatalogCase {
            case: "a logo payload",
            repository: "catalog/logo",
            // `logoImageBlob` is a blob, which awsJson1_1 carries base64-encoded.
            catalog_data: Some(json!({
                "description": "with a logo",
                "logoImageBlob": "aVZCT1J3MEtHZ29BQUE=",
            })),
        },
    ]
}

#[tokio::test]
async fn get_repository_catalog_data_reports_what_the_create_persisted() {
    let router = service();

    for case in catalog_cases() {
        let label = case.case;
        let mut request = json!({"repositoryName": case.repository});
        if let Some(catalog_data) = case.catalog_data.clone() {
            request["catalogData"] = catalog_data;
        }
        let created = ok(&router, "CreateRepository", request, label).await;

        let read = ok(
            &router,
            "GetRepositoryCatalogData",
            json!({"repositoryName": case.repository}),
            label,
        )
        .await;
        // The read reports exactly the record the create persisted — the whole point of
        // the operation, and what no amount of echoing could fake.
        assert_eq!(
            read["catalogData"], created["catalogData"],
            "case {label:?}: expected the catalog data CreateRepository stored ({}), got {}",
            created["catalogData"], read["catalogData"]
        );
        // Every stated member comes back under its own name.
        for (member, value) in case
            .catalog_data
            .as_ref()
            .and_then(Value::as_object)
            .map(|object| object.iter().collect::<Vec<_>>())
            .unwrap_or_default()
        {
            if member == "logoImageBlob" {
                // Not a member of the OUTPUT shape: a stored logo is reported as the
                // `logoUrl` it is reachable at.
                assert!(
                    read["catalogData"]["logoUrl"].is_string(),
                    "case {label:?}: expected a logoUrl for a stored logo, got {}",
                    read["catalogData"]
                );
                continue;
            }
            assert_eq!(
                &read["catalogData"][member], value,
                "case {label:?}: expected {member} to be reported as stored, got {}",
                read["catalogData"]
            );
        }
    }
}

#[tokio::test]
async fn get_repository_catalog_data_refuses_a_repository_that_does_not_exist() {
    let router = service();
    for repository in ["absent", "team/absent", "a.b_c-d/absent.one"] {
        let body = refused(
            &router,
            "GetRepositoryCatalogData",
            json!({"repositoryName": repository}),
            "RepositoryNotFoundException",
            repository,
        )
        .await;
        let message = error_message(&body);
        assert!(
            message.contains(repository) && message.contains(REGISTRY_ID),
            "case {repository:?}: expected the refusal to name the repository and the \
             registry, got {message:?}"
        );
    }
    // And a repository that DOES exist answers, so the refusal is about the repository
    // rather than about the operation.
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "absent"}),
        "the same name, created",
    )
    .await;
    ok(
        &router,
        "GetRepositoryCatalogData",
        json!({"repositoryName": "absent"}),
        "the same name, created",
    )
    .await;
}
