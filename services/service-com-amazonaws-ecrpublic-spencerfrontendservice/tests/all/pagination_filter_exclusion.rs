// SPDX-License-Identifier: CC0-1.0

//! Paging and naming the resources you want are mutually exclusive on an ECR Public
//! reader, driven end to end over the wire.
//!
//! The rule under test: a paginated reader whose input also takes an explicit identifier
//! list refuses a request that carries BOTH with HTTP 400 `InvalidParameterException` —
//! the one client-input error this model declares — stating this service's
//! parameter-constraint template over the pagination parameter and the rule it broke,
//! `Invalid parameter at '<Pagination>' failed to satisfy constraint: 'Cannot provide
//! <Pagination> if providing <List>'`, with nothing from the request interpolated. Source:
//! the model's own documentation of each pagination member, via
//! `smithy_query(query="doc", shape="DescribeRepositoriesRequest", member="maxResults")`
//! and its `nextToken` and `DescribeImagesRequest` counterparts — "If you specify
//! repositories with `repositoryNames`, you can't use this option" / "If you specify images
//! with `imageIds`, you can't use this option".
//!
//! The rows differ along what the rule generalises over: which reader is called
//! (`DescribeRepositories` over `repositoryNames`, `DescribeImages` over `imageIds`), which
//! pagination parameter is supplied beside the list (`maxResults`, `nextToken`, both), the
//! page size supplied (both published `@range(1..1000)` bounds and a value between them),
//! and whether the cursor is one this service minted. Either input ALONE is served, which
//! is what makes the refusal about the combination rather than about either member.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_message;
use crate::repository_image_lifecycle::{digest_of, ok, refused, service};

/// The repository the image reader is pointed at.
const REPOSITORY: &str = "pages/exclusion";

/// The clause this service states for a pagination parameter supplied beside a list,
/// built from the two parameter names and the rule rather than copied from a recording.
fn expected_message(pagination: &str, list: &str) -> String {
    format!(
        "Invalid parameter at '{pagination}' failed to satisfy constraint: \
         'Cannot provide {pagination} if providing {list}'"
    )
}

/// Seed two repositories, the first holding two tagged manifests, and answer the digest of
/// one of them so a request can name an image.
async fn seed(router: &MultiServiceRouter) -> String {
    for name in [REPOSITORY, "pages/neighbour"] {
        ok(
            router,
            "CreateRepository",
            json!({"repositoryName": name}),
            "seed",
        )
        .await;
    }
    let mut digest = String::new();
    for index in 0..2 {
        let manifest = json!({"schemaVersion": 2, "name": format!("page-{index}")}).to_string();
        ok(
            router,
            "PutImage",
            json!({
                "repositoryName": REPOSITORY,
                "imageManifest": manifest,
                "imageTag": format!("v{index}"),
            }),
            "seed",
        )
        .await;
        digest = digest_of(&manifest);
    }
    digest
}

/// One reader under test: the operation, the request members it always needs, the list
/// member it takes and the parameter name a refusal states for that list.
struct Reader {
    operation: &'static str,
    base: Value,
    list_member: &'static str,
    list_value: Value,
    list_parameter: &'static str,
}

fn readers(digest: &str) -> Vec<Reader> {
    vec![
        Reader {
            operation: "DescribeRepositories",
            base: json!({}),
            list_member: "repositoryNames",
            list_value: json!([REPOSITORY, "pages/neighbour"]),
            list_parameter: "RepositoryNames",
        },
        Reader {
            operation: "DescribeImages",
            base: json!({"repositoryName": REPOSITORY}),
            list_member: "imageIds",
            list_value: json!([{"imageDigest": digest}]),
            list_parameter: "ImageIds",
        },
    ]
}

#[tokio::test]
async fn a_pagination_parameter_beside_a_name_list_is_refused_naming_both_parameters() {
    let router = service();
    let digest = seed(&router).await;

    // The page sizes are both published `maxResults` bounds and one in between; the
    // cursors are one this service mints and one it never issued, so the refusal is shown
    // not to depend on the cursor being usable.
    let paginations: Vec<(&str, Value, &str)> = vec![
        ("the smallest page size", json!({"maxResults": 1}), "MaxResults"),
        ("a page size in the middle", json!({"maxResults": 100}), "MaxResults"),
        ("the largest page size", json!({"maxResults": 1000}), "MaxResults"),
        (
            "a cursor",
            json!({"nextToken": hex::encode(REPOSITORY.as_bytes())}),
            "NextToken",
        ),
        (
            "a cursor this service never minted",
            json!({"nextToken": "page 2 of 5?"}),
            "NextToken",
        ),
        (
            // Both pagination parameters at once: the page size is the one the clause
            // names, and the cursor's own validity is never reached.
            "a page size and a cursor together",
            json!({"maxResults": 2, "nextToken": "page 2 of 5?"}),
            "MaxResults",
        ),
    ];

    for reader in readers(&digest) {
        for (form, pagination, parameter) in &paginations {
            let case = format!("{}, {form}", reader.operation);
            let mut body = reader.base.clone();
            body[reader.list_member] = reader.list_value.clone();
            for (member, value) in pagination.as_object().expect("a request fragment") {
                body[member.as_str()] = value.clone();
            }
            let parsed = refused(
                &router,
                reader.operation,
                body,
                "InvalidParameterException",
                &case,
            )
            .await;
            let actual = error_message(&parsed);
            let expected = expected_message(parameter, reader.list_parameter);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn either_input_on_its_own_is_still_served() {
    let router = service();
    let digest = seed(&router).await;

    for reader in readers(&digest) {
        // The list alone: an explicit set of resources, reported unpaged.
        let mut named = reader.base.clone();
        named[reader.list_member] = reader.list_value.clone();
        let case = format!("{}, the list alone", reader.operation);
        ok(&router, reader.operation, named, &case).await;

        // The pagination parameters alone: a scan the caller walks a page at a time.
        for (form, pagination) in [
            ("a page size alone", json!({"maxResults": 1})),
            (
                "a page size and a cursor alone",
                json!({"maxResults": 1, "nextToken": hex::encode("v0".as_bytes())}),
            ),
        ] {
            let mut paged = reader.base.clone();
            for (member, value) in pagination.as_object().expect("a request fragment") {
                paged[member.as_str()] = value.clone();
            }
            let case = format!("{}, {form}", reader.operation);
            ok(&router, reader.operation, paged, &case).await;
        }
    }
}
