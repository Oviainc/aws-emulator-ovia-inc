// SPDX-License-Identifier: CC0-1.0

//! What a REFUSED mutation of the Serverless Application Repository leaves behind, end to
//! end over HTTP: nothing.
//!
//! The rule under test is the second half of the template contract the sibling
//! `version_templates` suite states: a publish whose template is not a packaged AWS SAM
//! template is refused, and because it is refused the store is exactly as it was — no
//! version in the application's version list, no latest version moved, no semantic version
//! consumed, and, on the create that publishes the first version, no application in the
//! catalogue and its name still free.
//!
//! The harness and the template helpers are the sibling suites', so every case here drives
//! the same wire path and judges a refusal by the same three requirements (status,
//! `errorCode`, a message built from the request).

use serde_json::json;

use super::applications::{created, path_label, Repo};
use super::version_templates::{
    assert_template_refusal, listed_names, listed_versions, plain_cloudformation, sam_template,
    SAM_TRANSFORM,
};

/// A refused publish leaves the application exactly as it was — the version list, the
/// latest version, and the semantic version itself still free to be published properly.
#[tokio::test]
async fn a_refused_publish_leaves_the_application_as_it_was() {
    let repo = Repo::new();
    let application_id = created(&repo, "rejected-publish-app").await;

    let first = repo
        .create_version(&application_id, "1.0.0", sam_template("First"))
        .await;
    assert_eq!(first.status, 201, "the first publish: got {}", first.body);

    let refused = repo
        .create_version(
            &application_id,
            "2.0.0",
            json!({ "templateBody": plain_cloudformation().to_string() }),
        )
        .await;
    assert_template_refusal(
        "a Transform-less publish after a good one",
        &refused,
        Some("AWS::SQS::Queue"),
    );

    assert_eq!(
        listed_versions(&repo, &application_id).await,
        vec!["1.0.0".to_string()],
        "only the published version is listed"
    );
    let fetched = repo.get_application(&application_id, None).await;
    assert_eq!(
        fetched.at("version").get("semanticVersion"),
        Some(&json!("1.0.0")),
        "the latest version is untouched by the refusal, got {}",
        fetched.body
    );

    // The refusal consumed nothing: the same semantic version publishes once it carries a
    // SAM template.
    let retried = repo
        .create_version(&application_id, "2.0.0", sam_template("Second"))
        .await;
    assert_eq!(
        retried.status, 201,
        "the same version publishes with a SAM template, got {} with {}",
        retried.status, retried.body
    );
    assert_eq!(
        listed_versions(&repo, &application_id).await,
        vec!["1.0.0".to_string(), "2.0.0".to_string()],
        "both published versions are listed"
    );
}

/// The same rule on the other operation that publishes a version: `CreateApplication` with
/// a `SemanticVersion` creates the first version from the same template contract, so a
/// Transform-less body is refused there too — and the application is not created either.
#[tokio::test]
async fn creating_an_application_with_a_transform_less_template_creates_nothing() {
    let repo = Repo::new();
    let name = "transform-less-create";
    let answer = repo
        .create_application(json!({
            "name": name,
            "author": "an-author",
            "description": "an application whose first version is not a SAM template",
            "semanticVersion": "1.0.0",
            "templateBody": plain_cloudformation().to_string()
        }))
        .await;
    assert_template_refusal(
        "a create whose first version is plain CloudFormation",
        &answer,
        Some("AWS::SQS::Queue"),
    );

    let application_id =
        format!("arn:aws:serverlessrepo:us-west-2:000000000000:applications/{name}");
    let fetched = repo.get_application(&application_id, None).await;
    assert_eq!(
        fetched.status, 404,
        "a refused create stores no application, got {} with {}",
        fetched.status, fetched.body
    );
    let names = listed_names(&repo).await;
    assert!(
        !names.iter().any(|listed| listed == name),
        "a refused create leaves the catalogue alone, got {names:?}"
    );

    // And the name is still free: the refusal reserved nothing.
    let accepted = repo
        .create_application(json!({
            "name": name,
            "author": "an-author",
            "description": "an application whose first version is a SAM template",
            "semanticVersion": "1.0.0",
            "templateBody": json!({
                "Transform": SAM_TRANSFORM,
                "Parameters": { "QueueVisibility": { "Type": "Number", "Default": 30 } },
                "Resources": {
                    "Q": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": { "VisibilityTimeout": { "Ref": "QueueVisibility" } }
                    }
                }
            })
            .to_string()
        }))
        .await;
    assert_eq!(
        accepted.status, 201,
        "the same name publishes with a SAM template, got {} with {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.at("version").get("semanticVersion"),
        Some(&json!("1.0.0")),
        "the first version is created with the application, got {}",
        accepted.body
    );
    assert_eq!(
        accepted.at("version")["parameterDefinitions"][0]["name"],
        json!("QueueVisibility"),
        "the accepted template's parameters are read out of it, got {}",
        accepted.at("version")
    );
    // ROUND TRIP, through the ARN a client percent-encodes into the URI label.
    let fetched = repo.get_application(&application_id, Some("1.0.0")).await;
    assert_eq!(
        fetched.status, 200,
        "the created application reads back at {}, got {} with {}",
        path_label(&application_id),
        fetched.status,
        fetched.body
    );
    assert_eq!(
        fetched.at("version")["parameterDefinitions"],
        accepted.at("version")["parameterDefinitions"],
        "the stored parameter definitions are what the create reported, got {}",
        fetched.body
    );
}
