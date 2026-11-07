// SPDX-License-Identifier: CC0-1.0

//! How this data plane WORDS the conflict a create of a name the directory already holds
//! is refused with, end to end over HTTP.
//!
//! Every case goes through the router: `POST <route>?DirectoryId=<id>` with a JSON body,
//! the same path a client takes (the harness is the validation suite's, reached as
//! `crate::validation_envelope::<item>`).
//!
//! RULE: a create whose `SAMAccountName` already names an object of that class in the
//! directory is refused with the modelled `ConflictException` (HTTP 409) stating the CLASS
//! of object that already exists and the directory it exists in — and the object already
//! stored is left exactly as it was. The name the request sent is NOT part of the sentence:
//! what a caller classifies the refusal by is the class, so a duplicate user and a
//! duplicate group are told apart by that and by nothing else.
//!
//! The table spans what the rule generalises over: both creates of this API (`CreateUser`,
//! `CreateGroup`), every spelling of a stored name the case-insensitive lookup folds
//! together, and two directories — so the sentence follows the class and the directory the
//! request named rather than the recorded name.

use serde_json::{json, Value};

use crate::validation_envelope::{call, emulator, Answer, DIRECTORY, OTHER_DIRECTORY};

/// The route of each create, with the class the refusal reports for it and the member a
/// stored object is read back by.
struct CreateCase {
    case: &'static str,
    create_route: &'static str,
    describe_route: &'static str,
    object_class: &'static str,
}

/// Both creates this API serves.
fn create_rows() -> [CreateCase; 2] {
    [
        CreateCase {
            case: "CreateUser",
            create_route: "/Users/CreateUser",
            describe_route: "/Users/DescribeUser",
            object_class: "User",
        },
        CreateCase {
            case: "CreateGroup",
            create_route: "/Groups/CreateGroup",
            describe_route: "/Groups/DescribeGroup",
            object_class: "Group",
        },
    ]
}

/// The spellings of one stored name that all name the same object: an AD account name is
/// case insensitive, so each is a duplicate of the stored one.
fn spellings(stored: &str) -> Vec<String> {
    vec![
        stored.to_string(),
        stored.to_uppercase(),
        stored.to_lowercase(),
    ]
}

/// The whole answer of a create that must be refused as a conflict.
fn assert_conflict(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_duplicate_name_is_refused_by_object_class_and_leaves_the_original_alone() {
    let router = emulator();

    for row in create_rows() {
        for directory in [DIRECTORY, OTHER_DIRECTORY] {
            let stored_name = format!("dup.{}", row.object_class);
            let seeding = format!("{} in {directory}", row.case);
            let created = call(
                &router,
                row.create_route,
                directory,
                &json!({ "SAMAccountName": &stored_name }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {seeding:?}: expected the first create to be served, got {} with {:?}",
                created.status, created.body
            );

            // What the stored object reports BEFORE any refused create: every refusal below
            // must leave this answer identical.
            let before = described(&router, row.describe_route, directory, &stored_name).await;

            for spelling in spellings(&stored_name) {
                let case = format!("{} in {directory} again as {spelling:?}", row.case);
                // The second create asks for different attributes, so a refusal that wrote
                // anything would show up in the read below.
                let again = call(
                    &router,
                    row.create_route,
                    directory,
                    &json!({
                        "SAMAccountName": &spelling,
                        "OtherAttributes": { "description": { "S": "the second create" } },
                    }),
                )
                .await;
                assert_conflict(&again, &case);
                assert_eq!(
                    again.message(),
                    Some(
                        format!(
                            "{class} already exists in directory {directory}.",
                            class = row.object_class
                        )
                        .as_str()
                    ),
                    "case {case:?}: unexpected diagnostic, got {:?}",
                    again.body
                );
                // The class is what a caller classifies by: the name it sent is not in the
                // sentence.
                let message = again.message().unwrap_or_default().to_string();
                assert!(
                    !message.contains(&spelling),
                    "case {case:?}: expected the account name to be absent, got {message:?}"
                );

                // The object already stored is untouched: same answer, member for member.
                let after = described(&router, row.describe_route, directory, &stored_name).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the stored object unchanged, got {after}"
                );
            }

            // The SAME name in the OTHER directory is not a duplicate: the refusal is about
            // this directory's objects, which is what the sentence names.
            let elsewhere = if directory == DIRECTORY {
                OTHER_DIRECTORY
            } else {
                DIRECTORY
            };
            let fresh = format!("new.{}", row.object_class);
            let served = call(
                &router,
                row.create_route,
                elsewhere,
                &json!({ "SAMAccountName": &fresh }),
            )
            .await;
            assert_eq!(
                served.status, 200,
                "case {:?}: expected a fresh name in {elsewhere} to be served, got {} with {:?}",
                row.case, served.status, served.body
            );
        }
    }
}

/// The whole body a describe of `sam_account_name` answers, which the cases compare across
/// a refused create.
async fn described(
    router: &smithy_server_runtime::MultiServiceRouter,
    route: &str,
    directory: &str,
    sam_account_name: &str,
) -> Value {
    let answer = call(
        router,
        route,
        directory,
        &json!({ "SAMAccountName": sam_account_name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected {route} to report {sam_account_name} in {directory}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}
