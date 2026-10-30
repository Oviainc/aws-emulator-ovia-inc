// SPDX-License-Identifier: CC0-1.0

//! Deleting a Realtime script, over the wire: `DeleteScript` "permanently deletes the
//! script record", so the script the request names — by script id or by script ARN — is
//! gone from every later read of the store, the rest of the account's scripts are
//! untouched, and an identifier that names no stored script is refused with the modelled
//! `NotFoundException` without removing anything.
//!
//! The harness (`GameLift`, `error_message`) is the one `hosting_loop.rs` defines, and the
//! script helpers are the ones `scripts.rs` already drives `CreateScript` with.

use serde_json::{json, Value};

use crate::hosting_loop::{error_code, error_message, GameLift};
use crate::scripts::{caller_location, script_arn, script_id_of, zip_bytes, zip_file_member};

/// The one wording this slice refuses a script identifier naming nothing stored with,
/// shared by `DescribeScript`, `UpdateScript` and `DeleteScript`: the resource kind, then
/// the identifier the CALLER supplied, verbatim, in parentheses
/// (`state::script_not_found_message`).
fn script_not_found_message(selector: &str) -> String {
    format!("Script ({selector}) not found.")
}

/// Create one script of `index` in the store and report the stored `Script`.
async fn create_script(gamelift: &GameLift, index: usize, from_caller_bucket: bool, tags: usize) -> Value {
    let mut request = json!({"Name": format!("deletable-{index}"), "Version": format!("{index}.0")});
    if from_caller_bucket {
        request["StorageLocation"] = caller_location(&format!("scripts/deletable-{index}.zip"));
    } else {
        request["ZipFile"] = zip_file_member(&zip_bytes(32 + index));
    }
    if tags > 0 {
        request["Tags"] = Value::Array(
            (0..tags)
                .map(|tag| json!({"Key": format!("key-{tag}"), "Value": format!("value-{tag}")}))
                .collect(),
        );
    }
    gamelift.ok("CreateScript", request).await["Script"].clone()
}

/// The `Script` a describe reports for `selector`, asserting the call succeeded.
async fn describe_script(gamelift: &GameLift, selector: &str) -> Value {
    gamelift
        .ok("DescribeScript", json!({"ScriptId": selector}))
        .await["Script"]
        .clone()
}

/// `DeleteScript` removes the one script its identifier resolves to: the delete succeeds
/// with an empty body, every later read of that script answers the modelled
/// `NotFoundException`, and the other scripts in the store still read back exactly as they
/// did before.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// script (id or ARN), how many scripts the store holds, which of them the request deletes
/// (the only one, the first, a middle one, the last), which documented source provided the
/// script's zip file, how many tags the resource carries, and whether the script was
/// updated before being deleted.
#[tokio::test]
async fn deleting_a_script_removes_the_one_script_the_identifier_resolves_to() {
    struct Case {
        label: &'static str,
        /// How many scripts the store holds when the delete runs.
        store: usize,
        /// The index, within that store, of the script the request deletes.
        target: usize,
        /// Whether the request names the target by its ARN rather than by its id.
        by_arn: bool,
        /// Whether the target was created from the caller's own S3 bucket rather than
        /// from a local zip file.
        from_caller_bucket: bool,
        /// How many tags the target carries.
        tags: usize,
        /// Whether `UpdateScript` revised the target before the delete.
        updated: bool,
    }

    let cases = [
        Case {
            label: "the only script in the store, named by its id",
            store: 1,
            target: 0,
            by_arn: false,
            from_caller_bucket: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the only script in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            from_caller_bucket: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the first of three scripts, named by its id",
            store: 3,
            target: 0,
            by_arn: false,
            from_caller_bucket: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "a middle script of five, named by its ARN",
            store: 5,
            target: 2,
            by_arn: true,
            from_caller_bucket: false,
            tags: 1,
            updated: false,
        },
        Case {
            label: "the last of four scripts, named by its id",
            store: 4,
            target: 3,
            by_arn: false,
            from_caller_bucket: false,
            tags: 3,
            updated: false,
        },
        Case {
            label: "a script pulled from the caller's own S3 bucket, named by its id",
            store: 2,
            target: 1,
            by_arn: false,
            from_caller_bucket: true,
            tags: 0,
            updated: false,
        },
        Case {
            label: "a script pulled from the caller's own S3 bucket, named by its ARN",
            store: 2,
            target: 0,
            by_arn: true,
            from_caller_bucket: true,
            tags: 2,
            updated: false,
        },
        Case {
            label: "a script whose metadata and content were updated first",
            store: 3,
            target: 1,
            by_arn: false,
            from_caller_bucket: false,
            tags: 0,
            updated: true,
        },
        Case {
            label: "an updated script named by its ARN",
            store: 2,
            target: 1,
            by_arn: true,
            from_caller_bucket: true,
            tags: 1,
            updated: true,
        },
    ];

    for case in &cases {
        let label = case.label;
        // Each row gets its own store, so what survives the delete is only what this row
        // created.
        let gamelift = GameLift::new();
        let mut scripts = Vec::new();
        for index in 0..case.store {
            scripts.push(
                create_script(
                    &gamelift,
                    index,
                    case.from_caller_bucket && index == case.target,
                    if index == case.target { case.tags } else { 0 },
                )
                .await,
            );
        }

        if case.updated {
            let target_id = script_id_of(&scripts[case.target]);
            let updated = gamelift
                .ok(
                    "UpdateScript",
                    json!({
                        "ScriptId": &target_id,
                        "Name": "revised-before-deletion",
                        "ZipFile": zip_file_member(&zip_bytes(1024)),
                    }),
                )
                .await;
            scripts[case.target] = updated["Script"].clone();
        }

        let target_id = script_id_of(&scripts[case.target]);
        let target_arn = script_arn(&target_id);
        let selector = if case.by_arn {
            target_arn.clone()
        } else {
            target_id.clone()
        };

        // Every script reads back as stored before the delete, so what changes below is
        // the delete's doing and nothing else.
        for (index, script) in scripts.iter().enumerate() {
            let described = describe_script(&gamelift, &script_id_of(script)).await;
            assert_eq!(
                &described, script,
                "case {label:?}: script {index} must read back as created, got {described}"
            );
        }

        let (status, body) = gamelift
            .call("DeleteScript", &json!({"ScriptId": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored script must succeed, got HTTP {status} with body {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteScript reports no members, got {body}"
        );

        // The deleted record is gone from the one store every read of the script is served
        // from: both identifier forms, the describe path and the update path.
        for probe in [target_id.clone(), target_arn.clone()] {
            for operation in ["DescribeScript", "UpdateScript", "DeleteScript"] {
                let mut request = json!({"ScriptId": &probe});
                if operation == "UpdateScript" {
                    request["Name"] = json!("after-deletion");
                }
                let (status, body) = gamelift.call(operation, &request).await;
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "NotFoundException"),
                    "case {label:?}: {operation} naming the deleted {probe:?} must be refused, \
                     got {status} {code} with body {body}"
                );
                let message = error_message(&body);
                assert_eq!(
                    message,
                    script_not_found_message(&probe),
                    "case {label:?}: {operation} naming the deleted {probe:?} must report the \
                     identifier the request carried, got {message:?}"
                );
            }
        }

        // Every other script of the store is untouched — member for member, by id and by
        // ARN.
        for (index, script) in scripts.iter().enumerate() {
            if index == case.target {
                continue;
            }
            let survivor_id = script_id_of(script);
            for probe in [survivor_id.clone(), script_arn(&survivor_id)] {
                let described = describe_script(&gamelift, &probe).await;
                assert_eq!(
                    &described, script,
                    "case {label:?}: script {index} must survive the delete unchanged, got {described}"
                );
            }
        }
    }
}

/// `DeleteScript` acts on ONE named resource, so an identifier that resolves to no stored
/// script is the modelled `NotFoundException` rather than an empty success — and the
/// scripts that ARE stored are left exactly as they were.
#[tokio::test]
async fn deleting_a_script_that_is_not_stored_is_refused_and_removes_nothing() {
    let gamelift = GameLift::new();
    let stored = create_script(&gamelift, 0, false, 1).await;
    let stored_id = script_id_of(&stored);

    // A script created and then deleted: its identifier is well formed and resolves to
    // nothing, exactly like one that never existed.
    let deleted = create_script(&gamelift, 1, false, 0).await;
    let deleted_id = script_id_of(&deleted);
    gamelift
        .ok("DeleteScript", json!({"ScriptId": &deleted_id}))
        .await;

    // Rows differ in the form of the identifier (bare id, UUID-shaped id, ARN), in whether
    // the script never existed or was deleted, and in which account and region the ARN
    // names — each well formed against the modelled `ScriptIdOrArn`
    // (`^(script-\S+|arn:.*:script\/script-\S+)$`).
    let cases: Vec<(&str, String)> = vec![
        ("an id no script carries", "script-does-not-exist".to_string()),
        (
            "a UUID-shaped id no script carries",
            "script-00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an ARN no script carries",
            script_arn("script-does-not-exist"),
        ),
        ("the id of a deleted script", deleted_id.clone()),
        ("the ARN of a deleted script", script_arn(&deleted_id)),
        (
            "an ARN in another account for a stored id",
            format!("arn:aws:gamelift:us-west-2:999999999999:script/{stored_id}"),
        ),
        (
            "an ARN in another region for a stored id",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:script/{stored_id}",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (label, selector) in &cases {
        let (status, body) = gamelift
            .call("DeleteScript", &json!({"ScriptId": selector}))
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an identifier naming no stored script must be refused, got {status} \
             {code} with body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            script_not_found_message(selector),
            "case {label:?}: the refusal must name the identifier the request carried, got \
             {message:?}"
        );
    }

    // None of those refusals removed anything: the stored script still reads back as
    // created.
    let described = describe_script(&gamelift, &stored_id).await;
    assert_eq!(
        described, stored,
        "the stored script must survive every refused delete, got {described}"
    );
}

/// A script record "lives until `DeleteScript` removes it", so the account's script
/// collection really is one shorter afterwards: with the quota full, the next create is
/// refused, and one delete makes room for exactly one more create.
#[tokio::test]
async fn a_delete_frees_a_slot_in_the_accounts_script_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_SCRIPTS_PER_ACCOUNT;
    let gamelift = GameLift::new();
    let mut ids = Vec::new();
    for index in 0..quota {
        let created = gamelift
            .ok(
                "CreateScript",
                json!({
                    "Name": format!("quota-{index}"),
                    "ZipFile": zip_file_member(&zip_bytes(8)),
                }),
            )
            .await;
        ids.push(script_id_of(&created["Script"]));
    }

    let (status, code) = gamelift
        .err(
            "CreateScript",
            json!({"Name": "before-the-delete", "ZipFile": zip_file_member(&zip_bytes(8))}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConflictException"),
        "a create with the collection full must be refused, got {status} {code}"
    );

    gamelift
        .ok("DeleteScript", json!({"ScriptId": &ids[0]}))
        .await;

    let created = gamelift
        .ok(
            "CreateScript",
            json!({"Name": "after-the-delete", "ZipFile": zip_file_member(&zip_bytes(8))}),
        )
        .await;
    let fresh_id = script_id_of(&created["Script"]);
    assert_ne!(
        fresh_id, ids[0],
        "the create after a delete must mint a NEW script id, got the deleted {fresh_id:?}"
    );

    // The collection is full again: the slot the delete freed took exactly one script.
    let (status, code) = gamelift
        .err(
            "CreateScript",
            json!({"Name": "one-too-many-again", "ZipFile": zip_file_member(&zip_bytes(8))}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConflictException"),
        "one delete must free exactly one slot, got {status} {code}"
    );

    // The scripts the delete did not name are still there.
    let described = describe_script(&gamelift, &ids[1]).await;
    assert_eq!(
        described["Name"],
        json!("quota-1"),
        "a script the delete did not name must be unchanged, got {described}"
    );
}
