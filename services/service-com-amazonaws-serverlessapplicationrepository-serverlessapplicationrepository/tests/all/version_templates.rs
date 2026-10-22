// SPDX-License-Identifier: CC0-1.0

//! What the Serverless Application Repository accepts as the template of a published
//! version, end to end over HTTP.
//!
//! The rule under test, asserted over a SET of templates rather than one recorded body: a
//! published version carries "the raw packaged AWS SAM template of your application"
//! (`smithy_query(query="doc", shape="CreateApplicationVersionRequest",
//! member="TemplateBody")`), and AWS SAM is a CloudFormation *transform* — so a template
//! body this emulator reads is publishable only if it declares a non-empty `Transform`
//! field (CloudFormation allows one name or a list of them). One that declares none is
//! refused with the modelled `BadRequestException` — HTTP 400, `errorCode`
//! `InvalidRequestException`, a message built from the request that names the offending
//! resource and the missing field — and NOTHING is stored for it, on either operation that
//! publishes a version (`CreateApplicationVersion`, and `CreateApplication` when its
//! request names a `SemanticVersion`).
//!
//! The harness (`Repo`, `Answer`) is the `applications` suite's, so these cases drive the
//! same wire path: the router resolves this service from the request's SigV4 credential
//! scope, restJson1 routes on method + URI, and the handler answers out of the one shared
//! engine.

use serde_json::{json, Value};

use super::applications::{created, Answer, Repo};

/// The transform a packaged SAM template declares: SAM is a CloudFormation macro, and
/// `AWS::Serverless-2016-10-31` is its name.
pub(crate) const SAM_TRANSFORM: &str = "AWS::Serverless-2016-10-31";

/// A SAM template declaring one queue, as a `templateBody` request member.
pub(crate) fn sam_template(resource: &str) -> Value {
    json!({
        "templateBody": json!({
            "Transform": SAM_TRANSFORM,
            "Resources": { resource: { "Type": "AWS::SQS::Queue" } }
        })
        .to_string()
    })
}

/// The plain CloudFormation body — valid CloudFormation, no `Transform` — that AWS
/// refuses, as a `templateBody` request member.
pub(crate) fn plain_cloudformation() -> Value {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": { "Q": { "Type": "AWS::SQS::Queue" } }
    })
}

/// The semantic versions an application holds, in the order the list reports them.
pub(crate) async fn listed_versions(repo: &Repo, application_id: &str) -> Vec<String> {
    let listed = repo.list_versions(application_id, "?maxItems=100").await;
    assert_eq!(
        listed.status, 200,
        "listing versions: expected 200, got {} with {}",
        listed.status, listed.body
    );
    listed
        .at("versions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|version| {
            version
                .get("semanticVersion")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The names the catalogue reports.
pub(crate) async fn listed_names(repo: &Repo) -> Vec<String> {
    let listed = repo.list_applications("?maxItems=100").await;
    listed
        .at("applications")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|summary| summary.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Every refusal of a template asserts the same three things the rule fixes: the status,
/// the condition's `errorCode`, and a message built from the request.
pub(crate) fn assert_template_refusal(case: &str, answer: &Answer, quoted: Option<&str>) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "BadRequestException",
        "case {case:?}: expected the modelled BadRequestException, got {}",
        answer.body
    );
    assert_eq!(
        answer.at("errorCode"),
        &json!("InvalidRequestException"),
        "case {case:?}: the body's errorCode names the condition, got {}",
        answer.at("errorCode")
    );
    let message = answer.at("message").as_str().unwrap_or_default().to_string();
    assert!(
        message.contains("[template]") && message.contains("Transform field"),
        "case {case:?}: the refusal names the offending resource and the missing Transform \
         field, got {message:?}"
    );
    if let Some(quoted) = quoted {
        assert!(
            message.contains(quoted),
            "case {case:?}: the refusal is built from the request, which carries {quoted:?}, \
             got {message:?}"
        );
    }
}

/// THE RULE: a published template must declare the SAM transform.
///
/// The rows span the forms CloudFormation gives the field (one name, a one-element list, a
/// list of several) and the forms that declare none (absent, absent beside other
/// top-level fields, empty object, an empty name, an empty list, a value that is not a
/// name), plus the two bodies this emulator does not read and therefore does not judge: a
/// template named only by URL, which is never fetched, and no template at all.
#[tokio::test]
async fn a_published_template_must_declare_the_sam_transform() {
    let repo = Repo::new();
    let application_id = created(&repo, "transform-app").await;

    // (case, request body, accepted?, a token of the request the refusal must quote)
    let cases: Vec<(&str, Value, bool, Option<&str>)> = vec![
        (
            "plain CloudFormation with no Transform",
            json!({ "templateBody": plain_cloudformation().to_string() }),
            false,
            Some("AWS::SQS::Queue"),
        ),
        (
            "the SAM transform as a single name",
            sam_template("Q"),
            true,
            None,
        ),
        (
            "the SAM transform as a one-element list",
            json!({
                "templateBody": json!({
                    "Transform": [SAM_TRANSFORM],
                    "Resources": { "Q": { "Type": "AWS::SQS::Queue" } }
                })
                .to_string()
            }),
            true,
            None,
        ),
        (
            "a list of transforms, SAM among them",
            json!({
                "templateBody": json!({
                    "Transform": ["AWS::Include", SAM_TRANSFORM],
                    "Resources": {}
                })
                .to_string()
            }),
            true,
            None,
        ),
        (
            "a template whose only field is Parameters",
            json!({
                "templateBody": json!({ "Parameters": { "P": { "Type": "String" } } }).to_string()
            }),
            false,
            Some("Parameters"),
        ),
        (
            "an empty template",
            json!({ "templateBody": "{}" }),
            false,
            None,
        ),
        (
            "an empty Transform name",
            json!({ "templateBody": json!({ "Transform": "", "Resources": {} }).to_string() }),
            false,
            Some("Transform"),
        ),
        (
            "an empty Transform list",
            json!({ "templateBody": json!({ "Transform": [], "Resources": {} }).to_string() }),
            false,
            None,
        ),
        (
            "a Transform that is not a name",
            json!({ "templateBody": json!({ "Transform": 7, "Resources": {} }).to_string() }),
            false,
            None,
        ),
        (
            "a template named only by URL, which is never fetched and so never judged",
            json!({ "templateUrl": "https://example.com/template.yaml" }),
            true,
            None,
        ),
        ("no template at all", json!({}), true, None),
    ];

    for (index, (case, request, accepted, quoted)) in cases.iter().enumerate() {
        // One semantic version per row, so an accepted row can never be mistaken for a
        // refused one by conflicting with it.
        let semantic_version = format!("{}.0.0", index + 1);
        let answer = repo
            .create_version(&application_id, &semantic_version, request.clone())
            .await;
        let versions = listed_versions(&repo, &application_id).await;

        if *accepted {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected 201, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                versions.contains(&semantic_version),
                "case {case:?}: a published version is listed, got {versions:?}"
            );
            continue;
        }

        assert_template_refusal(case, &answer, *quoted);
        assert!(
            !versions.contains(&semantic_version),
            "case {case:?}: a refused publish leaves no version behind, got {versions:?}"
        );
        let absent = repo
            .get_application(&application_id, Some(&semantic_version))
            .await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: the refused version is absent, got {} with {}",
            absent.status, absent.body
        );
    }
}

