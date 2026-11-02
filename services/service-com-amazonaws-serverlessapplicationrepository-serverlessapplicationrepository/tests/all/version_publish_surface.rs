// SPDX-License-Identifier: CC0-1.0

//! The version-publish surface of the Serverless Application Repository, end to end over
//! HTTP: the publish that creates an application and its first version in ONE call, and
//! every operation that then reports that application's versions.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. an application's version COLLECTION is reported in ascending semantic-version
//!    precedence (semver 2.0.0 §11, the order the member's own documentation points at:
//!    "The semantic version of the application: https://semver.org/",
//!    `smithy_query(query="doc", shape="Version", member="SemanticVersion")`) — the same
//!    ordering that decides which version the application reports when a request names
//!    none, and NOT the text order of the version strings, which disagrees with it;
//! 2. `CreateApplication` carrying a `SemanticVersion` and a packaged SAM template
//!    ("Creates an application, optionally including an AWS SAM file to create the first
//!    application version in the same call") persists the application together with that
//!    first version, and every version-scoped operation then answers out of the stored
//!    record: `GetApplication` with and without a named version,
//!    `CreateApplicationVersion` for a further version, `ListApplicationVersions`, a
//!    re-publish of a version that already exists (409 `ConflictException` /
//!    `VersionAlreadyExistsException`) and a body that is not a SAM template (400
//!    `BadRequestException` / `InvalidRequestException`).
//!
//! The harness (`Repo`, `Answer`) is the `applications` suite's, so every case drives the
//! same wire path a `serverlessrepo` SDK client does: the router resolves this service from
//! the request's SigV4 credential scope, restJson1 routes on method + URI, and the handler
//! answers out of the one shared engine.

use serde_json::{json, Value};

use super::applications::{created, Repo};
use super::version_templates::{
    assert_template_refusal, listed_versions, plain_cloudformation, sam_template, SAM_TRANSFORM,
};

/// Every semantic version a paged walk of one application's versions sees, in the order
/// the pages reported them.
async fn walked_versions(repo: &Repo, application_id: &str, page_size: usize) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!("?maxItems={page_size}&nextToken={token}"),
            None => format!("?maxItems={page_size}"),
        };
        let answer = repo.list_versions(application_id, &query).await;
        assert_eq!(
            answer.status, 200,
            "page {pages} of a walk by {page_size}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        for summary in answer.at("versions").as_array().cloned().unwrap_or_default() {
            seen.push(
                summary["semanticVersion"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        pages += 1;
        match answer.at("nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            pages < 64,
            "a walk by {page_size} did not terminate after {pages} pages; saw {seen:?}"
        );
    }
    seen
}

/// RULE 1: the versions of one application are reported in ascending semantic-version
/// precedence, whatever order they were published in.
///
/// Every row is a set of versions whose precedence order DISAGREES with the order of
/// their text, and the rows differ along what the rule generalises over: which of the
/// three numeric fields runs past nine (text sorts `1.10.0` below `1.9.0`), a prerelease
/// against the release it precedes (text sorts it above), numeric against alphanumeric
/// prerelease identifiers, and build metadata, which precedence ignores.
#[tokio::test]
async fn a_version_collection_is_ordered_by_semantic_version_precedence() {
    let repo = Repo::new();

    // (case, the order the versions are published in, the order they must be reported in)
    let cases: [(&str, &[&str], &[&str]); 6] = [
        (
            "a minor field past nine, published newest first",
            &["1.10.1", "1.9.0", "1.10.0"],
            &["1.9.0", "1.10.0", "1.10.1"],
        ),
        (
            "a patch field past nine",
            &["0.9.9", "0.10.0", "0.9.10"],
            &["0.9.9", "0.9.10", "0.10.0"],
        ),
        (
            "a major field past nine",
            &["9.0.0", "10.0.0", "2.0.0"],
            &["2.0.0", "9.0.0", "10.0.0"],
        ),
        (
            "prereleases below the release they precede",
            &["1.0.0", "1.0.0-beta", "1.0.0-alpha.1", "1.0.0-alpha"],
            &["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-beta", "1.0.0"],
        ),
        (
            "numeric prerelease identifiers compared as numbers, below an alphanumeric one",
            &["2.0.0-rc.1", "2.0.0-11", "2.0.0-2", "2.0.0-1"],
            &["2.0.0-1", "2.0.0-2", "2.0.0-11", "2.0.0-rc.1"],
        ),
        (
            "build metadata, which precedence ignores, beside a prerelease of the same core",
            &["3.0.0+build.2", "3.0.0-rc.1"],
            &["3.0.0-rc.1", "3.0.0+build.2"],
        ),
    ];

    for (index, (case, published, expected)) in cases.iter().enumerate() {
        // One application per row, so the rows cannot observe each other's versions.
        let application_id = created(&repo, &format!("precedence-app-{index}")).await;
        for semantic_version in *published {
            let answer = repo
                .create_version(&application_id, semantic_version, sam_template("Q"))
                .await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: publishing {semantic_version:?} expected 201, got {} with {}",
                answer.status, answer.body
            );
        }

        let wanted: Vec<String> = expected.iter().map(|v| (*v).to_string()).collect();
        let listed = listed_versions(&repo, &application_id).await;
        assert_eq!(
            listed, wanted,
            "case {case:?}: expected the versions in precedence order {wanted:?}, got {listed:?}"
        );

        // The order belongs to the COLLECTION, not to one page: a walk at any page size
        // sees the same sequence, once each.
        for page_size in [1usize, 2, 100] {
            let walked = walked_versions(&repo, &application_id, page_size).await;
            assert_eq!(
                walked, wanted,
                "case {case:?}: a walk by {page_size} must see {wanted:?}, got {walked:?}"
            );
        }

        // One ordering, not two: the last version of the walk is the one the application
        // reports when the request names none.
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched
                .at("version")
                .get("semanticVersion")
                .and_then(Value::as_str),
            expected.last().copied(),
            "case {case:?}: the highest-precedence version is the one reported with no \
             SemanticVersion named, got {}",
            fetched.body
        );
    }
}

/// One row of the publish table: the application's free-text metadata, the template it
/// publishes (in one of the two forms a packaged SAM template is written in), the two
/// versions it publishes and the parameters that template declares.
struct Publish {
    case: &'static str,
    /// `Author` is plain `__string` in the model, so its value is free text.
    author: &'static str,
    labels: Value,
    /// The version `CreateApplication` publishes in the same call as the application.
    first_version: &'static str,
    /// A further version, published by `CreateApplicationVersion`, with higher
    /// precedence than `first_version`.
    second_version: &'static str,
    /// The `templateBody` member's value.
    template_body: String,
    parameter_definitions: Value,
}

/// The publish class the rule generalises over: the rows differ in the form the template
/// is written in, how many parameters it declares and how they are referenced, how many
/// labels the application carries, and the grammar of the two semantic versions (a patch
/// bump, a minor field past nine, and a release that supersedes its own prerelease).
fn publishes() -> Vec<Publish> {
    vec![
        Publish {
            case: "a JSON SAM template with one parameter a resource references",
            author: "Example Publisher",
            labels: json!(["demo", "sam"]),
            first_version: "1.0.0",
            second_version: "1.0.1",
            template_body: json!({
                "Transform": SAM_TRANSFORM,
                "Parameters": { "Visibility": { "Type": "Number", "Default": 30 } },
                "Resources": {
                    "Q": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": { "VisibilityTimeout": { "Ref": "Visibility" } }
                    }
                }
            })
            .to_string(),
            parameter_definitions: json!([{
                "name": "Visibility",
                "type": "Number",
                "defaultValue": "30",
                "referencedByResources": ["Q"]
            }]),
        },
        Publish {
            case: "a YAML SAM template with a parameter interpolated by a resource",
            author: "example-author",
            labels: json!(["yaml"]),
            first_version: "1.9.0",
            second_version: "1.10.0",
            template_body: format!(
                "Transform: {SAM_TRANSFORM}\n\
                 Parameters:\n  \
                   Prefix:\n    \
                     Type: String\n    \
                     Default: alpha\n\
                 Resources:\n  \
                   Writer:\n    \
                     Type: AWS::SQS::Queue\n    \
                     Properties:\n      \
                       QueueName: !Sub '${{Prefix}}-writer'\n"
            ),
            parameter_definitions: json!([{
                "name": "Prefix",
                "type": "String",
                "defaultValue": "alpha",
                "referencedByResources": ["Writer"]
            }]),
        },
        Publish {
            case: "a template declaring no parameters at all, released from its prerelease",
            author: "A Publisher, with punctuation",
            labels: json!([]),
            first_version: "2.0.0-rc.1",
            second_version: "2.0.0",
            template_body: json!({
                "Transform": [SAM_TRANSFORM],
                "Resources": { "Q": { "Type": "AWS::SQS::Queue" } }
            })
            .to_string(),
            parameter_definitions: json!([]),
        },
    ]
}

/// RULE 2: one call publishes the application AND its first version, and the whole
/// version surface then answers out of what was stored.
#[tokio::test]
async fn a_publish_in_one_call_is_readable_across_the_version_surface() {
    let repo = Repo::new();

    for (index, row) in publishes().iter().enumerate() {
        let case = row.case;
        let name = format!("published-app-{index}");
        let description = format!("the {index} publish of this table");
        let created = repo
            .create_application(json!({
                "name": name,
                "author": row.author,
                "description": description,
                "labels": row.labels,
                "semanticVersion": row.first_version,
                "templateBody": row.template_body
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the publish expected 201, got {} with {}",
            created.status, created.body
        );
        let application_id = created
            .at("applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            application_id.ends_with(&format!(":applications/{name}")),
            "case {case:?}: the ApplicationId is the ARN minted from the name, got \
             {application_id:?}"
        );
        for (member, expected) in [
            ("name", json!(name)),
            ("author", json!(row.author)),
            ("description", json!(description)),
            ("labels", row.labels.clone()),
            ("isVerifiedAuthor", json!(false)),
        ] {
            assert_eq!(
                created.at(member),
                &expected,
                "case {case:?}: {member} must be reported as stored, expected {expected}, got {}",
                created.at(member)
            );
        }
        assert!(
            !created
                .at("creationTime")
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: the application reports a CreationTime, got {}",
            created.body
        );
        assert_version(
            case,
            "the version CreateApplication published",
            created.at("version"),
            &application_id,
            row.first_version,
            &row.parameter_definitions,
        );

        // Read after write: the version the publish answered with is the version the
        // store holds, reported both with and without the version named.
        for (reader, named) in [
            ("GetApplication with no version named", None),
            (
                "GetApplication naming the published version",
                Some(row.first_version),
            ),
        ] {
            let fetched = repo.get_application(&application_id, named).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?} ({reader}): expected 200, got {} with {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.at("version"),
                created.at("version"),
                "case {case:?} ({reader}): the stored version is the one the publish \
                 answered with, expected {}, got {}",
                created.at("version"),
                fetched.at("version")
            );
        }

        // A further version of the same application, published by the operation that
        // publishes one on its own.
        let published = repo
            .create_version(
                &application_id,
                row.second_version,
                json!({ "templateBody": row.template_body }),
            )
            .await;
        assert_eq!(
            published.status, 201,
            "case {case:?}: publishing {:?} expected 201, got {} with {}",
            row.second_version, published.status, published.body
        );
        assert_version(
            case,
            "the version CreateApplicationVersion published",
            &published.body,
            &application_id,
            row.second_version,
            &row.parameter_definitions,
        );

        // The application now resolves to the higher-precedence version, and lists both.
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched
                .at("version")
                .get("semanticVersion")
                .and_then(Value::as_str),
            Some(row.second_version),
            "case {case:?}: the application resolves to the version with the highest \
             precedence, got {}",
            fetched.body
        );
        let listed = listed_versions(&repo, &application_id).await;
        assert_eq!(
            listed,
            vec![
                row.first_version.to_string(),
                row.second_version.to_string()
            ],
            "case {case:?}: both published versions are listed in precedence order, got \
             {listed:?}"
        );

        // A version that already exists is the modelled conflict, not a second publish:
        // the condition is named in the body, and the stored version is untouched.
        let again = repo
            .create_version(
                &application_id,
                row.second_version,
                json!({ "templateBody": row.template_body }),
            )
            .await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: re-publishing a version expected 409, got {} with {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {}",
            again.body
        );
        assert_eq!(
            again.at("errorCode"),
            &json!("VersionAlreadyExistsException"),
            "case {case:?}: the body's errorCode names the condition, got {}",
            again.at("errorCode")
        );
        let message = again.at("message").as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(row.second_version) && message.contains(&application_id),
            "case {case:?}: the conflict names the version and the application it is \
             already published for, got {message:?}"
        );

        // A body that is not a packaged SAM template is refused on the same path, and
        // leaves the published versions exactly as they were.
        let refused = repo
            .create_version(
                &application_id,
                "4.0.0",
                json!({ "templateBody": plain_cloudformation().to_string() }),
            )
            .await;
        assert_template_refusal(case, &refused, Some("AWS::SQS::Queue"));
        let absent = repo.get_application(&application_id, Some("4.0.0")).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: the refused version was never published, got {} with {}",
            absent.status, absent.body
        );
        let listed_after = listed_versions(&repo, &application_id).await;
        assert_eq!(
            listed_after, listed,
            "case {case:?}: a refused publish leaves the collection as it was {listed:?}, got \
             {listed_after:?}"
        );
    }
}

/// Every `Version` this surface reports carries the same members, from the version that
/// was published: the application it belongs to, its semantic version, a template locator,
/// a creation time, the parameters its template declares, the capabilities its resources
/// require, and whether its resources are supported.
fn assert_version(
    case: &str,
    reported_by: &str,
    version: &Value,
    application_id: &str,
    semantic_version: &str,
    parameter_definitions: &Value,
) {
    for (member, expected) in [
        ("applicationId", json!(application_id)),
        ("semanticVersion", json!(semantic_version)),
        ("parameterDefinitions", parameter_definitions.clone()),
        // None of the declared resource types requires an acknowledgement, so the list is
        // present and empty rather than absent.
        ("requiredCapabilities", json!([])),
        ("resourcesSupported", json!(true)),
    ] {
        assert_eq!(
            version.get(member),
            Some(&expected),
            "case {case:?} ({reported_by}): expected {member} {expected}, got {:?}",
            version.get(member)
        );
    }
    for member in ["creationTime", "templateUrl"] {
        assert!(
            version
                .get(member)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "case {case:?} ({reported_by}): {member} is required of every version, got {version}"
        );
    }
}
