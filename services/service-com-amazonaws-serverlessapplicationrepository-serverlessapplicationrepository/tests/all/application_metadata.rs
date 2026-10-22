// SPDX-License-Identifier: CC0-1.0

//! The application metadata members of the Serverless Application Repository, end to end
//! over HTTP: what `CreateApplication` accepts for them, what `UpdateApplication`
//! rewrites, and what the rest of the lifecycle then reads back.
//!
//! The rules under test, each over a SET of inputs rather than one request:
//!
//! 1. an input is refused only for a bound the service applies. The model declares no
//!    constraint trait on `Author` (`smithy_query(query="operation",
//!    operation="CreateApplication")` shows a plain `__string`), and of what the member's
//!    documentation states, only its LENGTH is applied: `Author` is free-form attribution
//!    text, so any text of 1..=127 characters is accepted, stored and echoed verbatim,
//!    and only a value outside that length is a `BadRequestException`;
//! 2. a name already in the catalogue is the modelled conflict: HTTP 409, the wire error
//!    type `ConflictException`, and an `errorCode` naming the condition rather than
//!    repeating the status;
//! 3. `UpdateApplication` rewrites the members its request carries and leaves every other
//!    one as stored, and the rewrite is what every later read answers with;
//! 4. an application with no version lists an EMPTY version list, which is not the same
//!    answer as an application that does not exist (404);
//! 5. a delete of a created application succeeds with 204 and is observable as absence.

use serde_json::{json, Value};

use super::applications::{created, minimal, path_label, Answer, Repo};

/// The `Author` values RULE 1 generalises over: the member's documented prose pattern is
/// lowercase-and-single-hyphen, and every row here violates it in a different way — a
/// space, uppercase, punctuation, a leading or trailing hyphen, a double hyphen,
/// non-ASCII text — plus both published length bounds (1 and 127 characters).
fn accepted_authors() -> Vec<(&'static str, String)> {
    vec![
        ("a space between words", "Example Publisher".to_string()),
        ("the recorded probe form", "sim2real probe".to_string()),
        ("uppercase", "EXAMPLE".to_string()),
        ("mixed case and a dot", "A. Example Jr.".to_string()),
        ("punctuation", "Example, Inc. (AWS) <ops@example.com>".to_string()),
        ("a leading hyphen", "-example".to_string()),
        ("a trailing hyphen", "example-".to_string()),
        ("a double hyphen", "ex--ample".to_string()),
        ("an underscore and digits", "team_42".to_string()),
        ("non-ASCII text", "Ünicode Authör ✈".to_string()),
        ("the documented minimum length of 1", "e".to_string()),
        ("the documented maximum length of 127", "a".repeat(127)),
        (
            "127 non-ASCII characters, the maximum counted in characters",
            "é".repeat(127),
        ),
    ]
}

/// RULE 1: every author in the table is accepted, persisted and echoed back verbatim by
/// the create and by every later read of the record.
#[tokio::test]
async fn any_author_within_the_documented_length_is_accepted_and_stored() {
    let repo = Repo::new();
    for (index, (case, author)) in accepted_authors().into_iter().enumerate() {
        let name = format!("author-app-{index}");
        let request = json!({
            "name": name,
            "author": author,
            "description": "an application whose author is under test"
        });
        let created = repo.create_application(request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("author").as_str(),
            Some(author.as_str()),
            "case {case:?}: the Author must come back as sent, got {}",
            created.at("author")
        );
        // The whole success body is present, not just the echoed author.
        for member in ["applicationId", "name", "description", "creationTime"] {
            assert!(
                !created.at(member).is_null(),
                "case {case:?}: member {member} must be present, got {}",
                created.body
            );
        }
        assert_eq!(
            created.at("isVerifiedAuthor"),
            &json!(false),
            "case {case:?}: a freshly published application has no verified author, got {}",
            created.at("isVerifiedAuthor")
        );

        let application_id = created
            .at("applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetApplication, got {} with {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("author").as_str(),
            Some(author.as_str()),
            "case {case:?}: the stored Author must be read back, got {}",
            fetched.at("author")
        );
    }
}

/// RULE 1, the other side: the length is the bound that IS applied, on the create and on
/// the patch alike, and nothing outside it is written.
#[tokio::test]
async fn an_author_outside_the_documented_length_is_refused() {
    let repo = Repo::new();
    // Each published bound, and one character past it.
    let cases = [
        ("empty (one past the minimum of 1)", String::new()),
        ("128 characters (one past the maximum of 127)", "a".repeat(128)),
        (
            "128 non-ASCII characters (one past the maximum of 127)",
            "é".repeat(128),
        ),
    ];

    for (case, author) in &cases {
        let answer = repo
            .create_application(json!({
                "name": "length-refused-app",
                "author": author,
                "description": "an author outside the documented length"
            }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {}",
            answer.body
        );
    }

    // Nothing was written by any refused create.
    let listed = repo.list_applications("").await;
    assert_eq!(
        listed.at("applications").as_array().map(Vec::len),
        Some(0),
        "a refused create must not have written anything, got {}",
        listed.body
    );

    // The same bound holds on the patch, and a refused patch leaves the record alone.
    let application_id = created(&repo, "patch-length-app").await;
    let stored_author = repo
        .get_application(&application_id, None)
        .await
        .at("author")
        .as_str()
        .unwrap_or_default()
        .to_string();
    for (case, author) in &cases {
        let answer = update(&repo, &application_id, json!({ "author": author })).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} on UpdateApplication: expected 400, got {} with {}",
            answer.status, answer.body
        );
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched.at("author").as_str(),
            Some(stored_author.as_str()),
            "case {case:?}: a refused patch must leave the stored Author alone, got {}",
            fetched.at("author")
        );
    }
}

/// RULE 2: a second create of a name already in the catalogue conflicts, whatever its
/// other members say — including an author the prose pattern would have refused.
#[tokio::test]
async fn a_duplicate_name_is_the_modelled_conflict() {
    let repo = Repo::new();
    let names = ["conflicting-app", "another-conflicting-app"];
    for name in names {
        let first = repo
            .create_application(json!({
                "name": name,
                "author": "First Publisher",
                "description": "the application that holds the name"
            }))
            .await;
        assert_eq!(
            first.status, 201,
            "case {name:?}: the first create expected 201, got {} with {}",
            first.status, first.body
        );
        let application_id = first.at("applicationId").as_str().unwrap_or_default();

        let cases: Vec<(&str, Value)> = vec![
            ("the identical request", minimal(name)),
            (
                "a different author and description",
                json!({
                    "name": name,
                    "author": "Someone Else Entirely",
                    "description": "a different description"
                }),
            ),
            (
                "with a first version",
                json!({
                    "name": name,
                    "author": "Third Party",
                    "description": "with a first version",
                    "semanticVersion": "1.2.3"
                }),
            ),
        ];
        for (case, request) in &cases {
            let again = repo.create_application(request.clone()).await;
            assert_eq!(
                again.status, 409,
                "case {name:?}/{case:?}: expected 409, got {} with {}",
                again.status, again.body
            );
            assert_eq!(
                again.error_code(),
                "ConflictException",
                "case {name:?}/{case:?}: expected the modelled ConflictException, got {}",
                again.body
            );
            assert_eq!(
                again.at("errorCode").as_str(),
                Some("ApplicationAlreadyExistsException"),
                "case {name:?}/{case:?}: errorCode must name the condition, got {}",
                again.body
            );
            assert!(
                again
                    .at("message")
                    .as_str()
                    .unwrap_or_default()
                    .contains(application_id),
                "case {name:?}/{case:?}: the message names the application in the way, got {}",
                again.body
            );
        }
    }

    // Each name is held by exactly one application; no conflict wrote a second.
    let listed = repo.list_applications("").await;
    assert_eq!(
        listed.at("applications").as_array().map(Vec::len),
        Some(names.len()),
        "a refused create must not have written anything, got {}",
        listed.body
    );
}

/// RULE 3: the patch rewrites the members it carries and only those, and every later read
/// answers with the rewritten record.
///
/// The rows differ along which members the patch carries, how many labels it rewrites to,
/// and whether the readme arrives as a link or inline.
#[tokio::test]
async fn a_patch_rewrites_the_members_it_carries_and_only_those() {
    let repo = Repo::new();
    let patches: Vec<(&str, Value)> = vec![
        ("the author alone", json!({ "author": "Rewritten Author" })),
        (
            "the description alone",
            json!({ "description": "a rewritten description" }),
        ),
        (
            "a home page url",
            json!({ "homePageUrl": "https://example.com/rewritten" }),
        ),
        (
            "a rewritten label list",
            json!({ "labels": ["rewritten", "labels"] }),
        ),
        ("an emptied label list", json!({ "labels": [] })),
        (
            "ten labels, the documented maximum",
            json!({ "labels": ["l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9"] }),
        ),
        (
            "a readme link",
            json!({ "readmeUrl": "https://example.com/README.md" }),
        ),
        ("an inline readme", json!({ "readmeBody": "# rewritten" })),
        (
            "every patchable member at once",
            json!({
                "author": "Everything At Once",
                "description": "every patchable member in one call",
                "homePageUrl": "https://example.com/everything",
                "labels": ["all", "of", "them"],
                "readmeUrl": "https://example.com/all/README.md"
            }),
        ),
    ];

    // The members a patch never names, which must survive every one of them.
    let untouched = ["applicationId", "name", "creationTime", "spdxLicenseId"];

    for (index, (case, patch)) in patches.iter().enumerate() {
        let name = format!("patched-app-{index}");
        let created = repo
            .create_application(json!({
                "name": name,
                "author": "Original Author",
                "description": "the original description",
                "labels": ["original"],
                "spdxLicenseId": "Apache-2.0"
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the create expected 201, got {} with {}",
            created.status, created.body
        );
        let application_id = created
            .at("applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        let patched = update(&repo, &application_id, patch.clone()).await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            patched.status, patched.body
        );

        // Every member the patch named now reads as the patch said.
        for (member, value) in patch.as_object().expect("a patch object") {
            if member == "readmeBody" {
                // An inline body is reported as the locator the service minted for it,
                // exactly as on the create; the body itself is not an output member.
                assert!(
                    patched
                        .at("readmeUrl")
                        .as_str()
                        .unwrap_or_default()
                        .contains(&name),
                    "case {case:?}: readmeUrl must locate the stored readmeBody, got {}",
                    patched.at("readmeUrl")
                );
                continue;
            }
            // Every other member, `labels` included, reads back as sent — an empty list
            // is reported present and empty, never absent.
            assert_eq!(
                patched.at(member),
                value,
                "case {case:?}: member {member} must read as the patch set it, got {}",
                patched.at(member)
            );
        }

        // Every member it did not name is still the created one.
        for member in untouched {
            assert_eq!(
                patched.at(member),
                created.at(member),
                "case {case:?}: member {member} was not patched and must be unchanged, got {}",
                patched.at(member)
            );
        }
        for member in ["author", "description", "labels"] {
            if patch.get(member).is_none() {
                assert_eq!(
                    patched.at(member),
                    created.at(member),
                    "case {case:?}: member {member} was not patched and must be unchanged, got {}",
                    patched.at(member)
                );
            }
        }

        // And the patch is what a later read answers with, member for member.
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetApplication, got {} with {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body, patched.body,
            "case {case:?}: GetApplication must answer with the patched record, got {}",
            fetched.body
        );
    }
}

/// RULE 3, continued: a patch of an application that does not exist is the modelled 404,
/// and a patch outside a documented bound is the modelled 400.
#[tokio::test]
async fn a_patch_of_an_absent_application_is_not_found() {
    let repo = Repo::new();
    let real = created(&repo, "present-app").await;
    let unknown = [
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/never-created",
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/present-app-x",
    ];
    for application_id in unknown {
        let answer = update(&repo, application_id, json!({ "author": "Nobody" })).await;
        assert_eq!(
            answer.status, 404,
            "case {application_id:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {application_id:?}: expected NotFoundException, got {}",
            answer.body
        );
    }

    // A label outside its documented character class is still refused on the patch.
    let refused = update(&repo, &real, json!({ "labels": ["white space"] })).await;
    assert_eq!(
        refused.status, 400,
        "a label outside ^[a-zA-Z0-9+\\-_:\\/@]+$ expected 400, got {} with {}",
        refused.status, refused.body
    );
    // As is a patch carrying both readme forms.
    let both = update(
        &repo,
        &real,
        json!({ "readmeBody": "text", "readmeUrl": "https://example.com/README.md" }),
    )
    .await;
    assert_eq!(
        both.status, 400,
        "both ReadmeBody and ReadmeUrl expected 400, got {} with {}",
        both.status, both.body
    );
}

/// RULES 4 and 5: the lifecycle the metadata hangs off — an application with no version
/// lists an empty version list, and a delete of a created application succeeds and is
/// observable as absence, while the same calls against an application that does not exist
/// are the modelled 404.
#[tokio::test]
async fn a_version_less_application_lists_no_versions_and_deletes() {
    let repo = Repo::new();
    // Rows differing in what the application carries: nothing optional, a human author
    // and a full metadata set.
    let requests: Vec<(&str, Value)> = vec![
        ("only the required members", minimal("empty-versions-app")),
        (
            "a human-readable author",
            json!({
                "name": "human-author-app",
                "author": "Example Publisher",
                "description": "an app with no version"
            }),
        ),
        (
            "every metadata member",
            json!({
                "name": "full-metadata-app",
                "author": "Full Metadata Publisher",
                "description": "an app with no version",
                "homePageUrl": "https://example.com/home",
                "labels": ["one", "two"],
                "spdxLicenseId": "MIT"
            }),
        ),
    ];

    for (case, request) in &requests {
        let created = repo.create_application(request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            created.status, created.body
        );
        let application_id = created
            .at("applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            created.at("version"),
            &Value::Null,
            "case {case:?}: no SemanticVersion means no version was created, got {}",
            created.at("version")
        );

        let listed = repo.list_versions(&application_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListApplicationVersions, got {} with {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.at("versions"),
            &json!([]),
            "case {case:?}: an application with no version lists an empty list, got {}",
            listed.body
        );

        let deleted = repo.delete_application(&application_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected 204 from DeleteApplication, got {} with {}",
            deleted.status, deleted.body
        );

        // The delete is observable as absence on every reader, and a second delete is
        // the modelled 404 rather than a silent success.
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a deleted application expected 404, got {} with {}",
            fetched.status, fetched.body
        );
        let listed_again = repo.list_versions(&application_id, "").await;
        assert_eq!(
            listed_again.status, 404,
            "case {case:?}: an absent application 404s rather than listing nothing, got {} \
             with {}",
            listed_again.status, listed_again.body
        );
        let deleted_again = repo.delete_application(&application_id).await;
        assert_eq!(
            deleted_again.status, 404,
            "case {case:?}: deleting an absent application expected 404, got {} with {}",
            deleted_again.status, deleted_again.body
        );
    }

    let listed = repo.list_applications("").await;
    assert_eq!(
        listed.at("applications").as_array().map(Vec::len),
        Some(0),
        "every created application was deleted, got {}",
        listed.body
    );
}

/// RULE 2, generalised: the `errorCode` member of an error body names the CONDITION that
/// was raised, never the HTTP status.
///
/// `error_bodies.rs` pins this rule for the paths that existed before this change; the
/// rows here re-assert it across the whole slice INCLUDING the patch, whose 404 is new
/// here, and span both statuses that cover more than one condition: a 409 is "this
/// application already exists" AND "this version already exists", a 404 is "no such
/// application" AND "no such version", so `errorCode` is the only place a client tells
/// them apart.
#[tokio::test]
async fn an_error_body_names_the_condition_not_the_http_status() {
    let repo = Repo::new();
    let application_id = created(&repo, "error-code-app").await;
    let absent = "arn:aws:serverlessrepo:us-west-2:000000000000:applications/absent-app";
    let published = repo
        .create_version(
            &application_id,
            "1.0.0",
            json!({ "templateUrl": "https://example.com/template.yaml" }),
        )
        .await;
    assert_eq!(
        published.status, 201,
        "publishing the version the conflict rows need: got {} with {}",
        published.status, published.body
    );

    let cases: Vec<(&str, u16, &str, Answer)> = vec![
        (
            "a name already in the catalogue",
            409,
            "ApplicationAlreadyExistsException",
            repo.create_application(minimal("error-code-app")).await,
        ),
        (
            "a version the application already has",
            409,
            "VersionAlreadyExistsException",
            repo.create_version(&application_id, "1.0.0", json!({})).await,
        ),
        (
            "GetApplication on an application that does not exist",
            404,
            "ApplicationNotFoundException",
            repo.get_application(absent, None).await,
        ),
        (
            "UpdateApplication on an application that does not exist",
            404,
            "ApplicationNotFoundException",
            update(&repo, absent, json!({ "author": "Nobody" })).await,
        ),
        (
            "DeleteApplication on an application that does not exist",
            404,
            "ApplicationNotFoundException",
            repo.delete_application(absent).await,
        ),
        (
            "ListApplicationVersions on an application that does not exist",
            404,
            "ApplicationNotFoundException",
            repo.list_versions(absent, "").await,
        ),
        (
            "CreateApplicationVersion on an application that does not exist",
            404,
            "ApplicationNotFoundException",
            repo.create_version(absent, "1.0.0", json!({})).await,
        ),
        (
            "a version the application does not have",
            404,
            "VersionNotFoundException",
            repo.get_application(&application_id, Some("9.9.9")).await,
        ),
        (
            "an Author outside its documented length",
            400,
            "InvalidRequestException",
            repo.create_application(json!({
                "name": "invalid-request-app",
                "author": "",
                "description": "an author outside the documented length"
            }))
            .await,
        ),
        (
            "a version string that is not a semantic version",
            400,
            "InvalidRequestException",
            repo.create_version(&application_id, "1.0", json!({})).await,
        ),
    ];

    for (case, status, error_code, answer) in &cases {
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("errorCode").as_str(),
            Some(*error_code),
            "case {case:?}: errorCode must name the condition, got {}",
            answer.body
        );
        assert!(
            !answer.at("message").as_str().unwrap_or_default().is_empty(),
            "case {case:?}: an error carries a message, got {}",
            answer.body
        );
    }
}

/// `PATCH /applications/{ApplicationId}`, with the ARN percent-encoded into one path
/// segment the way an AWS SDK sends it.
async fn update(repo: &Repo, application_id: &str, patch: Value) -> Answer {
    let path = format!("/applications/{}", path_label(application_id));
    repo.call("PATCH", &path, Some(patch)).await
}
