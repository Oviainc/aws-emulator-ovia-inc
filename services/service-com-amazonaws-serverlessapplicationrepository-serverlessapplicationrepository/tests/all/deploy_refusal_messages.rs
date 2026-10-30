// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusals the deploy and version-resolution paths answer with.
//!
//! A refusal's message is part of its contract: a client — a deploy wrapper printing it,
//! a runbook matching it — reads it to tell two conditions that share one status and one
//! `errorCode` apart. Two rules are asserted here, each over a TABLE of inputs rather
//! than one request:
//!
//! 1. a request naming a `SemanticVersion` the application has not published is refused
//!    404 `NotFoundException` / `VersionNotFoundException` with the sentence
//!    "Application <ApplicationId> does not have a version <SemanticVersion>" — fixed
//!    text, the two variables taken from the request in that order, no closing
//!    punctuation — on EVERY operation that resolves a named version: `GetApplication`,
//!    `CreateCloudFormationTemplate` and `CreateCloudFormationChangeSet`;
//! 2. a deploy path asked to act on an application that has published NO version at all
//!    is refused 400 `BadRequestException` / `InvalidRequestException` with that path's
//!    own FIXED sentence, which interpolates nothing from the request or the store — so
//!    the same sentence comes back whatever application, name or region the request
//!    carries — and the two deploy paths word it differently;
//! 3. a request naming a `TemplateId` no prepared template of the application carries is
//!    refused 404 `NotFoundException` / `TemplateNotFoundException` with the sentence
//!    "Template with id <TemplateId> can not be found." — fixed text whose ONLY variable
//!    part is the `TemplateId` the request itself supplied, so the sentence names no other
//!    resource — on EVERY operation that resolves a prepared template:
//!    `GetCloudFormationTemplate` and `CreateCloudFormationChangeSet`.
//!
//! Only this service is mounted: every case below is refused before the handler reaches
//! CloudFormation, so the refusal is this service's own answer.

use serde_json::{json, Value};

use crate::applications::{created, path_label, Answer, Repo};
use crate::cloudformation_templates::read_template;
use crate::version_templates::sam_template;

/// `CreateCloudFormationChangeSet`: the fixed sentence for an application with no
/// published version. Recorded AWS behaviour; the model specifies no message text.
const NO_VERSION_CHANGE_SET: &str =
    "Can't create cloudformation changeset from application with no version.";

/// `CreateCloudFormationTemplate`: the same precondition, in that path's own fixed
/// sentence. Recorded AWS behaviour; the model specifies no message text.
const NO_VERSION_TEMPLATE: &str =
    "CloudFormation template can't be created from application with no version.";

/// `POST /applications/{ApplicationId}/changesets`.
async fn create_change_set(repo: &Repo, application_id: &str, body: Value) -> Answer {
    let path = format!("/applications/{}/changesets", path_label(application_id));
    repo.call("POST", &path, Some(body)).await
}

/// `POST /applications/{ApplicationId}/templates`.
async fn create_template(repo: &Repo, application_id: &str, body: Value) -> Answer {
    let path = format!("/applications/{}/templates", path_label(application_id));
    repo.call("POST", &path, Some(body)).await
}

/// Which operation a row of RULE 1 drives. All three resolve a `SemanticVersion` the
/// request names, so all three meet the same absence.
#[derive(Debug, Clone, Copy)]
enum Resolver {
    GetApplication,
    CreateCloudFormationTemplate,
    CreateCloudFormationChangeSet,
}

/// Drive `resolver` against `application_id` for `semantic_version`.
async fn resolve(
    repo: &Repo,
    resolver: Resolver,
    application_id: &str,
    semantic_version: &str,
) -> Answer {
    match resolver {
        Resolver::GetApplication => {
            repo.get_application(application_id, Some(semantic_version))
                .await
        }
        Resolver::CreateCloudFormationTemplate => {
            create_template(
                repo,
                application_id,
                json!({ "semanticVersion": semantic_version }),
            )
            .await
        }
        Resolver::CreateCloudFormationChangeSet => {
            create_change_set(
                repo,
                application_id,
                json!({
                    "stackName": "refusal-wording-stack",
                    "semanticVersion": semantic_version
                }),
            )
            .await
        }
    }
}

/// RULE 1: the 404 for a version the application has not published names the application
/// first and the version second, in AWS's own fixed text, on every operation that
/// resolves a version.
///
/// The rows span the operation, the application (one carrying versions, one carrying
/// none), and the SHAPE of the version string the request names — a plain release, a
/// release with multi-digit fields, a prerelease, a build-metadata version, and (for the
/// two operations that do not pre-validate the member) a string that is no semantic
/// version at all — because the message takes the version string from the request
/// verbatim.
#[tokio::test]
async fn an_unpublished_version_is_refused_in_awss_own_words() {
    let repo = Repo::new();
    let published = created(&repo, "wording-published-app").await;
    for semantic_version in ["1.0.0", "2.1.3"] {
        let created_version = repo
            .create_version(&published, semantic_version, sam_template("Queue"))
            .await;
        assert_eq!(
            created_version.status, 201,
            "publishing {semantic_version}: expected 201, got {} with {}",
            created_version.status, created_version.body
        );
    }
    let versionless = created(&repo, "wording-versionless-app").await;

    // (case, operation, application, the version the request names)
    let cases: Vec<(&str, Resolver, &str, &str)> = vec![
        (
            "GetApplication: a version above every published one",
            Resolver::GetApplication,
            &published,
            "9.9.9",
        ),
        (
            "GetApplication: a version below every published one",
            Resolver::GetApplication,
            &published,
            "0.0.1",
        ),
        (
            "GetApplication: a prerelease of a published version",
            Resolver::GetApplication,
            &published,
            "2.1.3-alpha.1",
        ),
        (
            "GetApplication: a string that is no semantic version",
            Resolver::GetApplication,
            &published,
            "not-a-version",
        ),
        (
            "GetApplication: an application that published nothing",
            Resolver::GetApplication,
            &versionless,
            "1.0.0",
        ),
        (
            "CreateCloudFormationTemplate: a version that was never published",
            Resolver::CreateCloudFormationTemplate,
            &published,
            "9.9.9",
        ),
        (
            "CreateCloudFormationTemplate: multi-digit fields",
            Resolver::CreateCloudFormationTemplate,
            &published,
            "10.20.30",
        ),
        (
            "CreateCloudFormationTemplate: build metadata",
            Resolver::CreateCloudFormationTemplate,
            &published,
            "1.0.0+build.7",
        ),
        (
            "CreateCloudFormationTemplate: a string that is no semantic version",
            Resolver::CreateCloudFormationTemplate,
            &published,
            "not-a-version",
        ),
        (
            "CreateCloudFormationTemplate: an application that published nothing",
            Resolver::CreateCloudFormationTemplate,
            &versionless,
            "1.0.0",
        ),
        (
            "CreateCloudFormationChangeSet: a version that was never published",
            Resolver::CreateCloudFormationChangeSet,
            &published,
            "9.9.9",
        ),
        (
            "CreateCloudFormationChangeSet: a prerelease of a published version",
            Resolver::CreateCloudFormationChangeSet,
            &published,
            "2.1.3-beta.2",
        ),
        (
            "CreateCloudFormationChangeSet: build metadata",
            Resolver::CreateCloudFormationChangeSet,
            &published,
            "1.0.0+build.7",
        ),
        (
            "CreateCloudFormationChangeSet: an application that published nothing",
            Resolver::CreateCloudFormationChangeSet,
            &versionless,
            "1.0.0",
        ),
    ];

    for (case, resolver, application_id, semantic_version) in &cases {
        let answer = resolve(&repo, *resolver, application_id, semantic_version).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {}",
            answer.body
        );
        assert_eq!(
            answer.at("errorCode"),
            &json!("VersionNotFoundException"),
            "case {case:?}: the body's errorCode names the condition, got {}",
            answer.body
        );
        let expected =
            format!("Application {application_id} does not have a version {semantic_version}");
        assert_eq!(
            answer.at("message").as_str().unwrap_or_default(),
            expected.as_str(),
            "case {case:?}: expected {expected:?}, got {}",
            answer.at("message")
        );
    }
}

/// RULE 2: each deploy path refuses an application with no published version in its OWN
/// fixed sentence, which carries nothing of the request.
///
/// The rows are applications differing in the name the `ApplicationId` is built from —
/// one character, a long mixed-case name, a name full of hyphens — so an application
/// value that leaked into the message would differ from row to row; the sentence must
/// not.
#[tokio::test]
async fn a_versionless_application_is_refused_with_each_deploy_paths_fixed_sentence() {
    let repo = Repo::new();
    let names = ["a", "Versionless-APP-0123456789-abcdefghij", "a-b-c-d-e"];

    let mut change_set_messages: Vec<String> = Vec::new();
    let mut template_messages: Vec<String> = Vec::new();
    for name in names {
        let application_id = created(&repo, name).await;

        // (case, the answer, the fixed sentence that path answers with)
        let refusals = vec![
            (
                format!("CreateCloudFormationChangeSet on {name:?}"),
                create_change_set(
                    &repo,
                    &application_id,
                    json!({ "stackName": "versionless-stack", "capabilities": [] }),
                )
                .await,
                NO_VERSION_CHANGE_SET,
            ),
            (
                format!("CreateCloudFormationTemplate on {name:?}"),
                create_template(&repo, &application_id, json!({})).await,
                NO_VERSION_TEMPLATE,
            ),
        ];

        for (case, answer, expected) in &refusals {
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
                answer.body
            );
            let message = answer.at("message").as_str().unwrap_or_default();
            assert_eq!(
                message, *expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(&application_id),
                "case {case:?}: the sentence carries nothing of the request, got {message:?}"
            );
        }

        change_set_messages.push(refusals[0].1.at("message").to_string());
        template_messages.push(refusals[1].1.at("message").to_string());
    }

    // One sentence per path, the same for every application: a message built from the
    // request would vary with the rows above.
    change_set_messages.dedup();
    template_messages.dedup();
    assert_eq!(
        change_set_messages.len(),
        1,
        "the change-set sentence is fixed across applications, got {change_set_messages:?}"
    );
    assert_eq!(
        template_messages.len(),
        1,
        "the template sentence is fixed across applications, got {template_messages:?}"
    );
    assert_ne!(
        NO_VERSION_CHANGE_SET, NO_VERSION_TEMPLATE,
        "the two deploy paths word the same precondition differently"
    );

    // The refusal is conditioned on the application having NO version, not on the request
    // omitting `SemanticVersion`: the same request against an application that published
    // one prepares a template of its latest version.
    let published = created(&repo, "wording-latest-app").await;
    let created_version = repo
        .create_version(&published, "1.4.2", sam_template("Queue"))
        .await;
    assert_eq!(
        created_version.status, 201,
        "publishing 1.4.2: expected 201, got {} with {}",
        created_version.status, created_version.body
    );
    let prepared = create_template(&repo, &published, json!({})).await;
    assert_eq!(
        prepared.status, 201,
        "preparing a template of the latest version: expected 201, got {} with {}",
        prepared.status, prepared.body
    );
    assert_eq!(
        prepared.at("semanticVersion"),
        &json!("1.4.2"),
        "preparing a template of the latest version: expected 1.4.2, got {}",
        prepared.body
    );
}

/// Which operation a row of RULE 3 drives. Both resolve a `TemplateId` the request names
/// — `GetCloudFormationTemplate` reads the prepared template, `CreateCloudFormationChangeSet`
/// deploys it — so both meet the same absence and answer it with the same sentence.
#[derive(Debug, Clone, Copy)]
enum TemplateReader {
    GetCloudFormationTemplate,
    CreateCloudFormationChangeSet,
}

/// Drive `reader` against `template_id` under `application_id`.
async fn resolve_template(
    repo: &Repo,
    reader: TemplateReader,
    application_id: &str,
    template_id: &str,
) -> Answer {
    match reader {
        TemplateReader::GetCloudFormationTemplate => {
            read_template(repo, application_id, template_id).await
        }
        TemplateReader::CreateCloudFormationChangeSet => {
            create_change_set(
                repo,
                application_id,
                json!({ "stackName": "template-wording-stack", "templateId": template_id }),
            )
            .await
        }
    }
}

/// RULE 3: the 404 for a `TemplateId` no prepared template of the application carries is
/// AWS's own fixed sentence, interpolating the requested `TemplateId` and nothing else.
///
/// The rows span the forms a `TemplateId` arrives in — the all-zero UUID, a UUID-shaped id
/// this service never minted, the same id in upper case, a single character, a value that
/// is no UUID at all, and a long one — plus the id of a template this service really DID
/// prepare, for a different application, which is absent only in this context. Because the
/// requested id is the sentence's only variable part, every row's message must differ from
/// every other row's exactly as its id does, and no row's may name the application.
#[tokio::test]
async fn an_absent_prepared_template_is_refused_in_awss_own_words() {
    let repo = Repo::new();
    let application_id = created(&repo, "wording-template-app").await;
    let published_version = repo
        .create_version(&application_id, "1.0.0", sam_template("Queue"))
        .await;
    assert_eq!(
        published_version.status, 201,
        "publishing 1.0.0: expected 201, got {} with {}",
        published_version.status, published_version.body
    );

    // A template this service really prepared, for ANOTHER application: a `TemplateId`
    // that exists, and is still absent through the application below.
    let other_id = created(&repo, "wording-other-template-app").await;
    let other_version = repo
        .create_version(&other_id, "1.0.0", sam_template("Queue"))
        .await;
    assert_eq!(
        other_version.status, 201,
        "publishing the other application's 1.0.0: expected 201, got {} with {}",
        other_version.status, other_version.body
    );
    let other_prepared = create_template(&repo, &other_id, json!({})).await;
    assert_eq!(
        other_prepared.status, 201,
        "preparing the other application's template: expected 201, got {} with {}",
        other_prepared.status, other_prepared.body
    );
    let other_template_id = other_prepared
        .at("templateId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the `TemplateId` the request names)
    let ids: Vec<(&str, String)> = vec![
        ("the all-zero UUID", "00000000-0000-0000-0000-000000000000".to_string()),
        (
            "a UUID-shaped id this service never minted",
            "0ba3f2a9-1e42-4b2a-9f2a-1c0d9e7b5a31".to_string(),
        ),
        (
            "that same id in upper case",
            "0BA3F2A9-1E42-4B2A-9F2A-1C0D9E7B5A31".to_string(),
        ),
        ("another application's prepared template", other_template_id),
        ("a value that is no UUID at all", "not-a-template-id".to_string()),
        ("a single character", "7".to_string()),
        ("a long id", "t".repeat(200)),
    ];

    let mut messages: Vec<String> = Vec::new();
    for (row, template_id) in &ids {
        for reader in [
            TemplateReader::GetCloudFormationTemplate,
            TemplateReader::CreateCloudFormationChangeSet,
        ] {
            let case = format!("{reader:?}: {row}");
            let answer = resolve_template(&repo, reader, &application_id, template_id).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotFoundException",
                "case {case:?}: expected the modelled NotFoundException, got {}",
                answer.body
            );
            assert_eq!(
                answer.at("errorCode"),
                &json!("TemplateNotFoundException"),
                "case {case:?}: the body's errorCode names the condition, got {}",
                answer.body
            );
            let expected = format!("Template with id {template_id} can not be found.");
            let message = answer.at("message").as_str().unwrap_or_default().to_string();
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The template is the only resource the caller got wrong, so it is the only
            // one the sentence names: neither the application it was asked for under nor
            // any ARN belongs in it.
            assert!(
                !message.contains(&application_id) && !message.contains("arn:"),
                "case {case:?}: the sentence names no resource but the template, got \
                 {message:?}"
            );
            messages.push(message);
        }
    }

    // One sentence per requested id, whichever operation answered it: the two operations
    // that resolve a prepared template word this absence identically.
    for pair in messages.chunks(2) {
        assert_eq!(
            pair.first(),
            pair.last(),
            "both operations word one absent template identically, got {pair:?}"
        );
    }
    messages.dedup();
    assert_eq!(
        messages.len(),
        ids.len(),
        "each requested id is echoed into its own sentence, got {messages:?}"
    );
}
