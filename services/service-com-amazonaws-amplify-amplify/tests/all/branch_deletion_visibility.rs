// SPDX-License-Identifier: CC0-1.0

//! Deletion visibility and the shape of Amplify's `NotFoundException`.
//!
//! Two rules are under test, both over the whole wire path
//! (`MultiServiceRouter::dispatch` on a restJson1 request signed for `amplify`), and
//! both over a TABLE rather than one recorded request:
//!
//! 1. A delete removes the addressed resource from persisted state and answers with
//!    the record as it existed at delete time; every later reader of that resource
//!    answers the modelled `NotFoundException` (HTTP 404, `x-amzn-errortype:
//!    NotFoundException`), and nothing else in state is disturbed.
//! 2. A `NotFoundException` message is built FROM THE REQUEST as a complete sentence:
//!    `<Entity> <identifier-from-the-request> not found.`, terminating period included.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{list_member, string_member, without_tags, Emulator, Reply};

/// The `NotFoundException` message the rule says a request addressing `identifier` of
/// kind `entity` must come back with.
fn expected_not_found_message(entity: &str, identifier: &str) -> String {
    format!("{entity} {identifier} not found.")
}

/// Assert `reply` is the modelled 404 for `entity`/`identifier`.
fn assert_not_found(case: &str, reply: &Reply, entity: &str, identifier: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404 for an absent record, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected the x-amzn-errortype header to name NotFoundException, got {:?}",
        reply.error_code
    );
    let expected = expected_not_found_message(entity, identifier);
    let actual = reply.message();
    assert_eq!(
        actual.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected the request-derived message {expected:?}, got {actual:?}"
    );
    assert!(
        actual.as_deref().is_some_and(|text| text.ends_with('.')),
        "case {case:?}: the message must be a complete sentence ending in a period, got {actual:?}"
    );
}

/// The branch-name table. `BranchName` is modelled `length 1..255, pattern ^(?s).+$`
/// (`smithy_query(query="operation",operation="DeleteBranch")`), so the rows span that
/// class: the modelled length minimum of 1, the maximum of 255, and the punctuation,
/// digits and mixed case the pattern admits — none of them the recorded `main`.
fn branch_name_cases() -> Vec<(&'static str, String)> {
    vec![
        ("shortest modelled name", "x".to_string()),
        ("release train name", "release-1.2.3".to_string()),
        ("underscored mixed case", "Feature_Login_v2".to_string()),
        ("dotted name", "env.staging".to_string()),
        ("digits only", "20260101".to_string()),
        ("name at the modelled length maximum", "b".repeat(255)),
    ]
}

/// The app-id table: ids that satisfy `AppId`'s modelled `length 1..20, pattern
/// ^d[a-z0-9]+$` and that no `CreateApp` in the test minted, so each reaches a handler
/// (not the boundary validator) and is absent from state.
fn absent_app_id_cases() -> Vec<(&'static str, String)> {
    vec![
        ("shortest modelled id", "d0".to_string()),
        ("alphanumeric id", "dnosuchapp1".to_string()),
        ("id at the modelled length maximum", format!("d{}", "9".repeat(19))),
    ]
}

#[tokio::test]
async fn delete_branch_removes_the_branch_and_every_later_read_answers_not_found() {
    let emulator = Emulator::new();

    for (label, name) in branch_name_cases() {
        let app_id = emulator.app_id_of_new_app(&format!("app-for-{label}")).await;

        // A sibling branch exists throughout: deleting one branch may not touch it.
        let sibling = "keeper";
        for branch in [name.as_str(), sibling] {
            let created = emulator
                .create_branch(&app_id, &json!({ "branchName": branch }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected HTTP 200 from CreateBranch, got {created:?}"
            );
        }

        let before = emulator.get_branch(&app_id, &name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: the branch must be readable before the delete, got {before:?}"
        );
        let before_record = before.member("branch");

        let deleted = emulator.delete_branch(&app_id, &name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected HTTP 200 from DeleteBranch, got {deleted:?}"
        );
        // A READ reports the resource's tag map and a WRITE echoes only the tags the
        // request set (asserted on its own in `app_branch_projection`), so the two
        // records are compared without that member.
        assert_eq!(
            without_tags(&deleted.member("branch")),
            without_tags(&before_record),
            "case {label:?}: DeleteBranch must answer the record as it existed at delete \
             time, got {:?}",
            deleted.member("branch")
        );

        // The deletion is visible: the reader answers the modelled 404...
        assert_not_found(
            &format!("{label}: GetBranch after DeleteBranch"),
            &emulator.get_branch(&app_id, &name).await,
            "Branch",
            &name,
        );
        // ...and so does a second delete of the same branch.
        assert_not_found(
            &format!("{label}: DeleteBranch twice"),
            &emulator.delete_branch(&app_id, &name).await,
            "Branch",
            &name,
        );

        // ListBranches reads the same map, so the deleted branch is gone from it while
        // the sibling is untouched.
        let listed = emulator.list_branches(&app_id, "").await;
        let names: Vec<String> = list_member(&listed.body, "branches")
            .iter()
            .map(|branch| string_member(branch, "branchName"))
            .collect();
        assert_eq!(
            names,
            vec![sibling.to_string()],
            "case {label:?}: only the undeleted branch may remain, got {names:?}"
        );
        assert_eq!(
            emulator.get_branch(&app_id, sibling).await.status,
            200,
            "case {label:?}: deleting one branch must not disturb another"
        );

        // The owning app is untouched by a branch delete.
        assert_eq!(
            emulator.get_app(&app_id).await.status,
            200,
            "case {label:?}: deleting a branch must not delete its app"
        );
    }
}

#[tokio::test]
async fn a_deleted_branch_name_is_free_again_and_the_recreated_branch_reads_back() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("recreate-app").await;

    for (label, name) in branch_name_cases() {
        let first = emulator
            .create_branch(&app_id, &json!({ "branchName": name, "stage": "BETA" }))
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected HTTP 200 from the first CreateBranch, got {first:?}"
        );
        assert_eq!(
            emulator.delete_branch(&app_id, &name).await.status,
            200,
            "case {label:?}: expected HTTP 200 from DeleteBranch"
        );

        // The record really left state, so the name is creatable again rather than
        // refused as a duplicate.
        let again = emulator
            .create_branch(
                &app_id,
                &json!({ "branchName": name, "stage": "PRODUCTION" }),
            )
            .await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: a deleted branch name must be creatable again, got {again:?}"
        );

        // And the re-created record is what a reader gets back, field for field.
        let read_back = emulator.get_branch(&app_id, &name).await;
        assert_eq!(
            read_back.status, 200,
            "case {label:?}: the re-created branch must be readable, got {read_back:?}"
        );
        assert_eq!(
            without_tags(&read_back.member("branch")),
            without_tags(&again.member("branch")),
            "case {label:?}: GetBranch must answer the re-created record, got {:?}",
            read_back.member("branch")
        );
        assert_eq!(
            string_member(&read_back.member("branch"), "stage"),
            "PRODUCTION",
            "case {label:?}: the re-created branch must carry the second request's stage"
        );

        // Leave state as this row found it.
        assert_eq!(
            emulator.delete_branch(&app_id, &name).await.status,
            200,
            "case {label:?}: expected HTTP 200 from the cleanup DeleteBranch"
        );
    }
}

#[tokio::test]
async fn every_not_found_message_names_the_entity_and_the_requested_identifier() {
    let emulator = Emulator::new();
    let existing_app = emulator.app_id_of_new_app("the-only-app").await;

    // An absent app: every operation addressed to it reports the APP as not found,
    // whatever the identifier looks like.
    for (label, app_id) in absent_app_id_cases() {
        let replies: Vec<(String, Reply)> = vec![
            (format!("{label}: GetApp"), emulator.get_app(&app_id).await),
            (
                format!("{label}: DeleteApp"),
                emulator.delete_app(&app_id).await,
            ),
            (
                format!("{label}: ListBranches"),
                emulator.list_branches(&app_id, "").await,
            ),
            (
                format!("{label}: CreateBranch"),
                emulator
                    .create_branch(&app_id, &json!({ "branchName": "main" }))
                    .await,
            ),
            (
                format!("{label}: GetBranch"),
                emulator.get_branch(&app_id, "main").await,
            ),
            (
                format!("{label}: DeleteBranch"),
                emulator.delete_branch(&app_id, "main").await,
            ),
        ];
        for (case, reply) in replies {
            assert_not_found(&case, &reply, "App", &app_id);
        }
    }

    // An existing app, an absent branch: the BRANCH is what the message names, and the
    // identifier is the one the request used.
    for (label, name) in branch_name_cases() {
        for (operation, reply) in [
            ("GetBranch", emulator.get_branch(&existing_app, &name).await),
            (
                "DeleteBranch",
                emulator.delete_branch(&existing_app, &name).await,
            ),
        ] {
            assert_not_found(&format!("{label}: {operation}"), &reply, "Branch", &name);
        }
    }

    // A deleted app reports itself absent for every operation too, which is the same
    // rule one level up from the branch.
    let doomed = emulator.app_id_of_new_app("doomed-app").await;
    assert_eq!(
        emulator.delete_app(&doomed).await.status,
        200,
        "case \"DeleteApp\": expected HTTP 200"
    );
    for (operation, reply) in [
        ("GetApp after DeleteApp", emulator.get_app(&doomed).await),
        (
            "DeleteBranch after DeleteApp",
            emulator.delete_branch(&doomed, "main").await,
        ),
    ] {
        assert_not_found(operation, &reply, "App", &doomed);
    }

    // The surviving app is still readable: a 404 path writes nothing.
    let survivor: Value = emulator.get_app(&existing_app).await.member("app");
    assert_eq!(
        string_member(&survivor, "appId"),
        existing_app,
        "case \"survivor\": an absent-record lookup must not disturb the app that exists"
    );
}
