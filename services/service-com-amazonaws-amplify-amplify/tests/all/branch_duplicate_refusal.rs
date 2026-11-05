// SPDX-License-Identifier: CC0-1.0

//! The shape of Amplify's duplicate-name refusal for `CreateBranch`.
//!
//! The rule under test, over the whole wire path (`MultiServiceRouter::dispatch` on a
//! restJson1 request signed for `amplify`): when a create is refused because the
//! owning app already holds a resource of that kind under the requested name, Amplify
//! answers the modelled `BadRequestException` (HTTP 400) whose message is built FROM
//! THE REQUEST's own members —
//! `Failed to create <entity>. The <entity> <name> already exists for the app <appId>`
//! — and the refused create leaves state exactly as it was.
//!
//! The rule generalises over the requested name (value, length, punctuation, case),
//! over the app the name already exists under, and over which optional members the
//! losing request carries, so each is a column of the table below rather than the one
//! recorded request. The harness (`Emulator`, `Reply`) is the one
//! `app_branch_lifecycle` already drives.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{list_member, string_member, without_tags, Emulator, Reply};

/// The message the rule says a refused create of `entity` named `identifier` under
/// `app_id` must come back with: a fixed prefix, the request's own name, a fixed
/// infix, and the request's own app id. The entity is lower case, as Amplify renders
/// it.
fn expected_already_exists_message(entity: &str, identifier: &str, app_id: &str) -> String {
    format!(
        "Failed to create {entity}. The {entity} {identifier} already exists for the app {app_id}"
    )
}

/// Assert `reply` is the modelled duplicate-name refusal for a branch named
/// `branch_name` under `app_id`.
fn assert_already_exists(case: &str, reply: &Reply, branch_name: &str, app_id: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400 for a duplicate name, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("BadRequestException"),
        "case {case:?}: expected the x-amzn-errortype header to name BadRequestException, got {:?}",
        reply.error_code
    );
    let expected = expected_already_exists_message("branch", branch_name, app_id);
    let actual = reply.message();
    assert_eq!(
        actual.as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected the request-derived message {expected:?}, got {actual:?}"
    );
    // The two request-derived parts appear in the template's order, so a caller
    // substring-matching the prefix finds the name after it and the app id last.
    let text = actual.unwrap_or_default();
    assert!(
        text.starts_with("Failed to create branch. The branch "),
        "case {case:?}: the message must open with the fixed prefix, got {text:?}"
    );
    assert!(
        text.ends_with(&format!(" already exists for the app {app_id}")),
        "case {case:?}: the message must close with the infix and the request's appId, \
         got {text:?}"
    );
}

/// The branch-name table. `BranchName` is modelled `length 1..255, pattern ^(?s).+$`
/// (`smithy_query(query="operation",operation="CreateBranch")`), so the rows span that
/// class: the modelled length minimum of 1, the maximum of 255, and the punctuation,
/// digits and mixed case the pattern admits — none of them the recorded
/// `dup-branch-<hex>`.
///
/// Every row here is also addressable in a `{branchName}` URI label, so a row can be
/// read back with `GetBranch` and removed with `DeleteBranch`. Names that need
/// percent-encoding to travel in a path are in [`body_only_branch_name_cases`].
fn branch_name_cases() -> Vec<(&'static str, String)> {
    vec![
        ("shortest modelled name", "x".to_string()),
        ("dotted release name", "release-1.2.3".to_string()),
        ("underscored mixed case", "Feature_Login_v2".to_string()),
        ("digits only", "20260101".to_string()),
        ("name at the modelled length maximum", "b".repeat(255)),
    ]
}

/// Branch names a URI label cannot carry unencoded, so they are exercised through
/// `CreateBranch`'s BODY and read back through `ListBranches` rather than through
/// `GetBranch`.
///
/// A branch name is drawn from letters, digits and the symbols `/_.-`
/// (`service_com_amazonaws_amplify_amplify::state::is_branch_name`), of which only `/`
/// has to be percent-encoded to travel in a path segment, so every row here is a
/// slashed name — at one, two and three segments.
fn body_only_branch_name_cases() -> Vec<(&'static str, String)> {
    vec![
        ("slashed feature name", "feature/login-v2".to_string()),
        ("dated release path", "release/2026/01".to_string()),
        ("single-character segments", "a/b".to_string()),
    ]
}

/// The losing request's optional members: the refusal depends only on the name and the
/// app, so a bare request and a fully specified one must be refused identically.
fn losing_request_shapes() -> Vec<(&'static str, Value)> {
    vec![
        ("only the required branchName", json!({})),
        ("with a stage", json!({ "stage": "PRODUCTION" })),
        (
            "with a stage, a description and environment variables",
            json!({
                "stage": "BETA",
                "description": "the second create",
                "displayName": "second",
                "environmentVariables": { "A": "1" },
            }),
        ),
    ]
}

/// `{ "branchName": name }` plus every member of `extra`.
fn create_request(name: &str, extra: &Value) -> Value {
    let mut request = json!({ "branchName": name });
    for (member, value) in extra.as_object().expect("a table row is an object") {
        request[member.as_str()] = value.clone();
    }
    request
}

/// How many branches of `name` the app holds, read through `ListBranches`.
async fn branches_named(emulator: &Emulator, app_id: &str, name: &str) -> usize {
    let listed = emulator.list_branches(app_id, "").await;
    assert_eq!(
        listed.status, 200,
        "ListBranches on {app_id}: expected HTTP 200, got {listed:?}"
    );
    list_member(&listed.body, "branches")
        .iter()
        .filter(|branch| string_member(branch, "branchName") == name)
        .count()
}

#[tokio::test]
async fn a_duplicate_branch_name_is_refused_with_the_request_derived_message() {
    let emulator = Emulator::new();

    // Two apps, so the `appId` the message names is the request's own and not just
    // "the only app in state".
    let apps = [
        emulator.app_id_of_new_app("duplicate-host-one").await,
        emulator.app_id_of_new_app("duplicate-host-two").await,
    ];

    for (label, name) in branch_name_cases() {
        for (app_index, app_id) in apps.iter().enumerate() {
            let created = emulator
                .create_branch(app_id, &json!({ "branchName": name, "stage": "DEVELOPMENT" }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?} on app {app_index}: expected HTTP 200 from the first \
                 CreateBranch, got {created:?}"
            );
            let stored = emulator.get_branch(app_id, &name).await;
            assert_eq!(
                stored.status, 200,
                "case {label:?} on app {app_index}: the created branch must be readable, \
                 got {stored:?}"
            );

            for (shape, extra) in losing_request_shapes() {
                let case = format!("{label} on app {app_index}, losing request {shape}");
                let refused = emulator
                    .create_branch(app_id, &create_request(&name, &extra))
                    .await;
                assert_already_exists(&case, &refused, &name, app_id);

                // The refused create wrote nothing: the branch a reader sees is still
                // the one the winning create stored, with its own stage and display
                // name, not the loser's.
                let after = emulator.get_branch(app_id, &name).await;
                assert_eq!(
                    without_tags(&after.member("branch")),
                    without_tags(&stored.member("branch")),
                    "case {case:?}: a refused create must leave the stored branch \
                     untouched, got {:?}",
                    after.member("branch")
                );
            }

            // Exactly one branch of that name exists under the app.
            let matching = branches_named(&emulator, app_id, &name).await;
            assert_eq!(
                matching, 1,
                "case {label:?} on app {app_index}: the app must hold exactly one branch \
                 of that name, got {matching}"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_holds_for_names_a_uri_label_cannot_carry() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("body-only-host").await;

    for (label, name) in body_only_branch_name_cases() {
        let created = emulator
            .create_branch(&app_id, &json!({ "branchName": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from the first CreateBranch, got {created:?}"
        );

        let refused = emulator
            .create_branch(&app_id, &json!({ "branchName": name, "stage": "BETA" }))
            .await;
        assert_already_exists(label, &refused, &name, &app_id);

        let matching = branches_named(&emulator, &app_id, &name).await;
        assert_eq!(
            matching, 1,
            "case {label:?}: the refused create must not add a second branch, got {matching}"
        );
    }
}

#[tokio::test]
async fn a_branch_name_taken_on_one_app_is_free_on_another() {
    let emulator = Emulator::new();
    let first = emulator.app_id_of_new_app("scope-app-one").await;
    let second = emulator.app_id_of_new_app("scope-app-two").await;

    for (label, name) in branch_name_cases() {
        for app_id in [&first, &second] {
            let created = emulator
                .create_branch(app_id, &json!({ "branchName": name }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: a name taken on another app must be creatable, got {created:?}"
            );
        }

        // ...and only then is the name a duplicate, on each app independently, with the
        // message naming that app.
        for app_id in [&first, &second] {
            let refused = emulator
                .create_branch(app_id, &json!({ "branchName": name }))
                .await;
            assert_already_exists(
                &format!("{label}: second create under {app_id}"),
                &refused,
                &name,
                app_id,
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_name_stops_being_a_duplicate() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("reuse-app").await;

    for (label, name) in branch_name_cases() {
        assert_eq!(
            emulator
                .create_branch(&app_id, &json!({ "branchName": name }))
                .await
                .status,
            200,
            "case {label:?}: expected HTTP 200 from the first CreateBranch"
        );
        assert_already_exists(
            &format!("{label}: duplicate before the delete"),
            &emulator
                .create_branch(&app_id, &json!({ "branchName": name }))
                .await,
            &name,
            &app_id,
        );
        assert_eq!(
            emulator.delete_branch(&app_id, &name).await.status,
            200,
            "case {label:?}: expected HTTP 200 from DeleteBranch"
        );

        // The refusal is computed from state, so once the record is gone the same
        // request succeeds instead of being refused again.
        let again = emulator
            .create_branch(&app_id, &json!({ "branchName": name }))
            .await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: a deleted name must stop being a duplicate, got {again:?}"
        );
    }
}
