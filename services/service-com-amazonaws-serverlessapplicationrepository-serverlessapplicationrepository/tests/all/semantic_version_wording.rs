// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal this service answers for a `SemanticVersion` that is not a
//! semantic version.
//!
//! The rule under test: a request whose `SemanticVersion` is not a semantic version
//! (<https://semver.org/>, the grammar the member's own documentation points at) is
//! refused 400 `BadRequestException` / `InvalidRequestException` with the service's FIXED
//! sentence for that validation failure — a sentence that reports the version grammar and
//! so substitutes nothing from the request or from the store: it names neither the
//! rejected value, nor the input member, nor the application. Because the modelled
//! `ErrorCode` is the one condition for every invalid parameter (`BadRequestException`:
//! `Message` documented "One of the parameters in the request is invalid",
//! `smithy_query(query="shape", shape="BadRequestException")`), that sentence is the only
//! thing that tells a client which parameter it got wrong.
//!
//! The rule holds for EVERY operation that validates the member — `CreateApplication`,
//! `CreateApplicationVersion` and `CreateCloudFormationChangeSet` — so every row below is
//! driven through all three, and the whole table must come back with one single sentence.
//!
//! Only this service is mounted: every case is refused before the handler reaches
//! CloudFormation, so the refusal is this service's own answer.

use serde_json::{json, Value};

use crate::applications::{created, Answer, Repo};

/// The fixed sentence a refused `SemanticVersion` comes back with. Recorded AWS
/// behaviour: the model carries no `@pattern` for the member and specifies no message
/// text, so the wording is the service's, and it is fixed — the "See … for syntax" clause
/// included.
const INVALID_SEMANTIC_VERSION: &str =
    "Version name is a not valid semantic version. See https://semver.org/ for syntax";

/// Which operation a row drives. All three take a `SemanticVersion` from the caller and
/// validate it against the same grammar, so all three answer the same condition.
#[derive(Debug, Clone, Copy)]
enum Validator {
    /// `CreateApplication`: `POST /applications`, with the version in the body.
    Application,
    /// `CreateApplicationVersion`: `PUT
    /// /applications/{ApplicationId}/versions/{SemanticVersion}`, with the version as the
    /// `@httpLabel` path segment.
    Version,
    /// `CreateCloudFormationChangeSet`: `POST /applications/{ApplicationId}/changesets`,
    /// with the version in the body.
    ChangeSet,
}

impl Validator {
    /// The operation this row drives, as the model names it.
    fn operation(self) -> &'static str {
        match self {
            Validator::Application => "CreateApplication",
            Validator::Version => "CreateApplicationVersion",
            Validator::ChangeSet => "CreateCloudFormationChangeSet",
        }
    }
}

/// Drive `validator` against `application_id` with `semantic_version`.
///
/// `name` is the application name a `CreateApplication` row publishes under; it is a legal
/// name, so the version is what the request gets refused for.
async fn validate(
    repo: &Repo,
    validator: Validator,
    application_id: &str,
    name: &str,
    semantic_version: &str,
) -> Answer {
    match validator {
        Validator::Application => {
            repo.create_application(json!({
                "name": name,
                "author": "example-author",
                "description": "an app created with a version under test",
                "semanticVersion": semantic_version
            }))
            .await
        }
        Validator::Version => {
            let path = format!(
                "/applications/{}/versions/{}",
                crate::applications::path_label(application_id),
                // The same single-segment encoder the application label uses: a version
                // under test can carry a space, and it has to reach the handler as written.
                crate::applications::path_label(semantic_version)
            );
            repo.call("PUT", &path, Some(json!({}))).await
        }
        Validator::ChangeSet => {
            let path = format!(
                "/applications/{}/changesets",
                crate::applications::path_label(application_id)
            );
            repo.call(
                "POST",
                &path,
                Some(json!({
                    "stackName": "semver-wording-stack",
                    "semanticVersion": semantic_version
                })),
            )
            .await
        }
    }
}

/// THE RULE: every operation that validates `SemanticVersion` refuses a value outside the
/// semver grammar with one fixed sentence, carrying nothing of the request.
///
/// The rows span the ways a value falls outside the grammar documented for the member
/// (semver 2.0.0 §2 and §9, the source `crate::state::is_semantic_version` states): too
/// few and too many numeric fields, a leading zero, a non-numeric field, an empty or
/// leading-zero prerelease identifier, an empty build identifier, a prefixed version, a
/// word that is no version at all, trailing whitespace and a leading sign — and each row
/// is driven through all three operations, which carry the member in two different places
/// (a path label and a body member). The recorded case (a two-field version on
/// `CreateApplicationVersion`) is one row of the table.
#[tokio::test]
async fn a_non_semantic_version_is_refused_with_one_fixed_sentence() {
    let repo = Repo::new();
    let application_id = created(&repo, "semver-wording-app").await;

    let invalid = [
        ("a single numeric field", "1"),
        ("two numeric fields", "1.0"),
        ("four numeric fields", "1.0.0.0"),
        ("a leading zero in a numeric field", "01.0.0"),
        ("non-numeric fields", "a.b.c"),
        ("an empty prerelease", "1.0.0-"),
        ("a leading zero in a numeric prerelease identifier", "1.0.0-01"),
        ("an empty prerelease identifier", "1.0.0-alpha..1"),
        ("an empty build identifier", "1.0.0+"),
        ("a prefixed version", "v1.0.0"),
        ("a word that is no version", "latest"),
        ("trailing whitespace", "1.0.0 "),
        ("a leading sign", "-1.0.0"),
    ];

    let mut sentences: Vec<String> = Vec::new();
    for (row, semantic_version) in invalid {
        for validator in [
            Validator::Application,
            Validator::Version,
            Validator::ChangeSet,
        ] {
            let case = format!(
                "{}: {row} ({semantic_version:?})",
                validator.operation()
            );
            let answer = validate(
                &repo,
                validator,
                &application_id,
                "semver-wording-created",
                semantic_version,
            )
            .await;
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
            let message = answer.at("message").as_str().unwrap_or_default().to_string();
            assert_eq!(
                message, INVALID_SEMANTIC_VERSION,
                "case {case:?}: expected {INVALID_SEMANTIC_VERSION:?}, got {message:?}"
            );
            // The sentence reports the grammar, so none of the request is in it: not the
            // rejected value, not the member name, not the application it was sent for.
            assert!(
                !message.contains(semantic_version.trim())
                    && !message.contains("SemanticVersion")
                    && !message.contains(&application_id),
                "case {case:?}: the sentence carries nothing of the request, got {message:?}"
            );
            sentences.push(message);
        }
    }

    // One sentence for the whole table: a message built from the request or from the
    // operation would differ from row to row.
    sentences.dedup();
    assert_eq!(
        sentences.len(),
        1,
        "every refused version comes back with one sentence, got {sentences:?}"
    );
}

/// The refusal is keyed on the GRAMMAR, not on the operation or the member's place: a
/// value that IS a semantic version gets past this validation on every one of the three
/// operations, and what it then meets is that operation's own condition — never the
/// grammar sentence.
#[tokio::test]
async fn a_semantic_version_is_never_refused_for_its_syntax() {
    let repo = Repo::new();
    let published = created(&repo, "semver-wording-accepts-app").await;

    // (case, the operation, the version it names, the status it must answer)
    let accepted: Vec<(&str, Validator, &str, u16)> = vec![
        // A create that names the first version, and a version added to an application
        // that already exists: both publish, so both answer 201.
        (
            "CreateApplication publishes the first version",
            Validator::Application,
            "1.0.0",
            201,
        ),
        (
            "CreateApplicationVersion adds a release",
            Validator::Version,
            "2.1.0",
            201,
        ),
        (
            "CreateApplicationVersion adds a prerelease",
            Validator::Version,
            "2.2.0-beta.1",
            201,
        ),
        (
            "CreateApplicationVersion adds a version with build metadata",
            Validator::Version,
            "10.20.30+build.7",
            201,
        ),
        // A well-formed version this application never published is an ABSENT version,
        // not an invalid one: 404, and a different sentence.
        (
            "CreateCloudFormationChangeSet on a version never published",
            Validator::ChangeSet,
            "9.9.9",
            404,
        ),
    ];

    for (case, validator, semantic_version, expected_status) in &accepted {
        let answer = validate(
            &repo,
            *validator,
            &published,
            "semver-wording-accepted",
            semantic_version,
        )
        .await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected {expected_status}, got {} with {}",
            answer.status, answer.body
        );
        let message = answer.at("message").as_str().unwrap_or_default().to_string();
        assert_ne!(
            message, INVALID_SEMANTIC_VERSION,
            "case {case:?}: a semantic version is never refused for its syntax, got {}",
            answer.body
        );
    }

    // ROUND TRIP: the versions the accepted rows published are readable back, so the rule
    // refuses only what the grammar refuses. `+` is encoded in the query string, where a
    // literal `+` means a space.
    for semantic_version in ["2.1.0", "2.2.0-beta.1", "10.20.30+build.7"] {
        let path = format!(
            "/applications/{}?semanticVersion={}",
            crate::applications::path_label(&published),
            semantic_version.replace('+', "%2B")
        );
        let read = repo.call("GET", &path, None).await;
        assert_eq!(
            read.status, 200,
            "reading {semantic_version:?} back: expected 200, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.at("version").get("semanticVersion"),
            Some(&Value::String(semantic_version.to_string())),
            "reading {semantic_version:?} back: expected it on the version, got {}",
            read.body
        );
    }
}
