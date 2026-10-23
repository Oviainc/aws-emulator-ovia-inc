// SPDX-License-Identifier: CC0-1.0

//! A member whose content the model documents as a JSON document is PARSED before it is
//! stored.
//!
//! The rule under test: ECR Public refuses a body that is not parseable JSON with 400
//! `InvalidParameterException`, whose message names the member and the rule it failed
//! (`Invalid parameter at '<Member>' failed to satisfy constraint: 'Invalid JSON
//! syntax'`) and does NOT quote the rejected body back, and the refused request stores
//! nothing. A body that IS parseable JSON is stored verbatim, whatever JSON value it is.
//!
//! Both members this service declares as JSON text are rows of the same table:
//! `PutImage.imageManifest` ("The image manifest that corresponds to the image to be
//! uploaded") and `SetRepositoryPolicy.policyText` ("The policy text will be in JSON
//! format"). The bodies span the ways a document can fail to parse — a truncated object,
//! an unterminated string, unquoted keys, trailing content, prose, a single `{` — and the
//! ways it can succeed — an object, an array, a string, a number, a deeply nested
//! document.

use serde_json::json;

use super::harness::error_message;
use crate::repository_image_lifecycle::{call, ok, refused, service};

/// What the service must answer for a member whose document does not parse: the member
/// it names, and the rule it failed.
fn expected_message(member: &str) -> String {
    format!(
        "Invalid parameter at '{member}' failed to satisfy constraint: 'Invalid JSON syntax'"
    )
}

/// Bodies that are not JSON documents at all.
fn malformed_bodies() -> Vec<(&'static str, String)> {
    vec![
        // The recorded instance: an object that is never closed.
        ("an unclosed object", "{not-valid-json-at-all".to_string()),
        ("a lone opening brace", "{".to_string()),
        ("prose", "this is not a manifest".to_string()),
        ("unquoted keys", "{schemaVersion: 2}".to_string()),
        ("a single quote string", "{'schemaVersion': 2}".to_string()),
        ("an unterminated string", "{\"schemaVersion\": \"2".to_string()),
        ("a trailing comma", "{\"schemaVersion\": 2,}".to_string()),
        ("trailing content", "{\"schemaVersion\": 2} and more".to_string()),
        ("a truncated array", "[1, 2".to_string()),
        ("a bare word", "nope".to_string()),
    ]
}

/// Bodies that ARE JSON documents, across the value kinds JSON admits.
fn well_formed_bodies() -> Vec<(&'static str, String)> {
    vec![
        ("an empty object", "{}".to_string()),
        (
            "an object",
            json!({"schemaVersion": 2, "layers": []}).to_string(),
        ),
        ("an array", json!([1, 2, 3]).to_string()),
        ("a string", json!("just text").to_string()),
        ("a number", "17".to_string()),
        (
            "a nested document",
            json!({"a": {"b": {"c": [{"d": true}]}}}).to_string(),
        ),
    ]
}

#[tokio::test]
async fn put_image_refuses_a_manifest_that_is_not_json_and_stores_nothing() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    for (case, manifest) in malformed_bodies() {
        let body = refused(
            &router,
            "PutImage",
            json!({
                "repositoryName": "team/app",
                "imageManifest": manifest,
                "imageTag": "broken",
            }),
            "InvalidParameterException",
            case,
        )
        .await;
        assert_eq!(
            error_message(&body),
            expected_message("ImageManifest"),
            "case {case:?}: expected the member and the rule — and no echo of the rejected \
             body — got {body}"
        );

        // Nothing was stored: no image, and no tag pointing at one.
        let described = ok(
            &router,
            "DescribeImages",
            json!({"repositoryName": "team/app"}),
            case,
        )
        .await;
        assert_eq!(
            described["imageDetails"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            0,
            "case {case:?}: a refused push must store no image, got {described}"
        );
        refused(
            &router,
            "DescribeImages",
            json!({
                "repositoryName": "team/app",
                "imageIds": [{"imageTag": "broken"}],
            }),
            "ImageNotFoundException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn put_image_stores_every_well_formed_json_manifest() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    for (index, (case, manifest)) in well_formed_bodies().into_iter().enumerate() {
        let pushed = ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": "team/app",
                "imageManifest": manifest,
                "imageTag": format!("v{index}"),
            }),
            case,
        )
        .await;
        assert_eq!(
            pushed["image"]["imageManifest"].as_str(),
            Some(manifest.as_str()),
            "case {case:?}: expected the manifest to be stored verbatim, got {pushed}"
        );
    }
}

#[tokio::test]
async fn set_repository_policy_refuses_a_policy_that_is_not_json_and_stores_nothing() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    for (case, policy) in malformed_bodies() {
        // The same rule on this service's other JSON-document member: a body that does
        // not parse is refused with the one client-input error this operation models.
        // (`SetRepositoryPolicy` then holds the parsed document to the IAM policy grammar
        // as well, which `tests/all/repository_policy.rs` covers; the member and the
        // constraint are what this row is about.)
        refused(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": "team/app", "policyText": policy}),
            "InvalidParameterException",
            case,
        )
        .await;

        // Nothing was attached: the repository still carries no policy.
        refused(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": "team/app"}),
            "RepositoryPolicyNotFoundException",
            case,
        )
        .await;
    }
}

/// Policy documents that ARE JSON and that the IAM policy grammar admits, so the only
/// question left is whether what was stored is what comes back.
fn well_formed_policies() -> Vec<(&'static str, String)> {
    vec![
        (
            "a one-statement policy",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowPull",
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "ecr-public:BatchGetImage",
                }],
            })
            .to_string(),
        ),
        (
            "a two-statement policy with list members",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": ["*"]},
                        "Action": [
                            "ecr-public:BatchGetImage",
                            "ecr-public:GetDownloadUrlForLayer",
                        ],
                    },
                    {
                        "Effect": "Allow",
                        "Principal": {"Service": "codebuild.amazonaws.com"},
                        "Action": "ecr-public:BatchGetImage",
                    },
                ],
            })
            .to_string(),
        ),
    ]
}

#[tokio::test]
async fn set_repository_policy_stores_every_well_formed_json_document() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    for (case, policy) in well_formed_policies() {
        ok(
            &router,
            "SetRepositoryPolicy",
            json!({"repositoryName": "team/app", "policyText": policy}),
            case,
        )
        .await;
        let read = ok(
            &router,
            "GetRepositoryPolicy",
            json!({"repositoryName": "team/app"}),
            case,
        )
        .await;
        assert_eq!(
            read["policyText"].as_str(),
            Some(policy.as_str()),
            "case {case:?}: expected the stored document, got {read}"
        );
    }
}

#[tokio::test]
async fn a_malformed_document_is_refused_before_the_repository_is_even_resolved() {
    // The refusal is about the document, so it does not depend on the rest of the request
    // being satisfiable: a malformed manifest for a repository that does not exist is
    // still the document's refusal, with no image created anywhere.
    let router = service();
    let (status, body) = call(
        &router,
        "PutImage",
        json!({
            "repositoryName": "team/never-created",
            "imageManifest": "{not-valid-json-at-all",
            "imageTag": "broken",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected 400 for a malformed manifest, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        expected_message("ImageManifest"),
        "expected the manifest's own refusal, got {body}"
    );
}
