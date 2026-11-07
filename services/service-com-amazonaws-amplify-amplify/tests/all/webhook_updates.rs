// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateWebhook`.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the JSON an AWS SDK
//! client parses, not an internal function's return value.
//!
//! The rule under test: an update of an existing resource stores every member the
//! request carries, leaves every member it omits as it was, re-mints no part of the
//! resource's identity, stamps the record's "last updated" timestamp with the instant
//! of the call, and answers the record it just wrote — so the next read answers the
//! same thing. A resource the store does not hold is the modelled 404 and stores
//! nothing.
//!
//! The table spans the class the rule generalises over: which of `branchName` and
//! `description` the request carries (neither, each alone, both), and the values those
//! members take across the modelled bounds — `description` empty, single-line,
//! multi-line and at its `length` maximum of 1000; `branchName` at one character, a
//! path-like name and at its `length` maximum of 255. The bounds come from
//! `smithy_query(query="operation",operation="UpdateWebhook")`.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, timestamp_member, Emulator};

/// The members of a webhook that an update must NEVER change: its identity, the app it
/// belongs to, and the instant it was created. The names are the wire names
/// `smithy_query(query="shape",shape="Webhook")` lists.
const IDENTITY_MEMBERS: [&str; 5] = ["appId", "createTime", "webhookArn", "webhookId", "webhookUrl"];

/// One `UpdateWebhook` request and the `branchName` / `description` the rule says the
/// webhook carries afterwards.
struct UpdateCase {
    case: &'static str,
    request: Value,
    branch_name: String,
    description: String,
}

/// The `UpdateWebhook` table, applied in order to ONE webhook, so each row also proves
/// that what the previous row stored survived.
///
/// The webhook starts out on branch `main` with the description `"the original"`.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "an empty update carries no member, so nothing changes",
            request: json!({}),
            branch_name: "main".to_string(),
            description: "the original".to_string(),
        },
        UpdateCase {
            case: "branchName alone leaves the description alone",
            request: json!({ "branchName": "develop" }),
            branch_name: "develop".to_string(),
            description: "the original".to_string(),
        },
        UpdateCase {
            case: "description alone leaves the branch alone",
            request: json!({ "description": "re-described, same branch" }),
            branch_name: "develop".to_string(),
            description: "re-described, same branch".to_string(),
        },
        UpdateCase {
            case: "both members at once",
            request: json!({
                "branchName": "feature/add-login",
                "description": "triggers the login preview build",
            }),
            branch_name: "feature/add-login".to_string(),
            description: "triggers the login preview build".to_string(),
        },
        UpdateCase {
            case: "description at the modelled length minimum (the empty string)",
            request: json!({ "description": "" }),
            branch_name: "feature/add-login".to_string(),
            description: String::new(),
        },
        UpdateCase {
            case: "a multi-line description, which the modelled pattern ^(?s) admits",
            request: json!({ "description": "nightly trigger\nowned by the release team" }),
            branch_name: "feature/add-login".to_string(),
            description: "nightly trigger\nowned by the release team".to_string(),
        },
        UpdateCase {
            case: "description at the modelled length maximum of 1000",
            request: json!({ "description": "d".repeat(1000) }),
            branch_name: "feature/add-login".to_string(),
            description: "d".repeat(1000),
        },
        UpdateCase {
            case: "branchName of one character, its modelled length minimum",
            request: json!({ "branchName": "x" }),
            branch_name: "x".to_string(),
            description: "d".repeat(1000),
        },
        UpdateCase {
            case: "branchName at the modelled length maximum of 255",
            request: json!({ "branchName": "b".repeat(255), "description": "longest branch" }),
            branch_name: "b".repeat(255),
            description: "longest branch".to_string(),
        },
        UpdateCase {
            case: "a branch name the app has no branch for",
            request: json!({ "branchName": "not-created-yet" }),
            branch_name: "not-created-yet".to_string(),
            description: "longest branch".to_string(),
        },
    ]
}

#[tokio::test]
async fn an_update_stores_what_it_carries_and_keeps_the_rest() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("webhook-update-host").await;
    // A second app's webhook exists throughout: updating one webhook must never change
    // what another app's webhook reads as.
    let other_app_id = emulator.app_id_of_new_app("webhook-update-bystander").await;
    let bystander = emulator
        .create_webhook(&other_app_id, &json!({ "branchName": "main" }))
        .await;
    assert_eq!(
        bystander.status, 200,
        "expected HTTP 200 for the bystander CreateWebhook, got {bystander:?}"
    );
    let bystander_id = string_member(&bystander.member("webhook"), "webhookId");
    let bystander_record = bystander.member("webhook");

    let created = emulator
        .create_webhook(
            &app_id,
            &json!({ "branchName": "main", "description": "the original" }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateWebhook, got {created:?}"
    );
    let created_record = created.member("webhook");
    let webhook_id = string_member(&created_record, "webhookId");
    let mut previous_update_time = timestamp_member(&created_record, "updateTime");

    for case in update_cases() {
        let label = case.case;
        let updated = emulator.update_webhook(&webhook_id, &case.request).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateWebhook, got {updated:?}"
        );
        let record = updated.member("webhook");

        assert_eq!(
            string_member(&record, "branchName"),
            case.branch_name,
            "case {label:?}: expected the branchName the rule gives, got {record:?}"
        );
        assert_eq!(
            string_member(&record, "description"),
            case.description,
            "case {label:?}: expected the description the rule gives, got {record:?}"
        );

        // The identity an update never re-mints, and the instant the webhook was
        // created, are what the create handed out.
        for member in IDENTITY_MEMBERS {
            assert_eq!(
                record.get(member),
                created_record.get(member),
                "case {label:?}: an update must not change {member:?}, got {:?} where the create \
                 answered {:?}",
                record.get(member),
                created_record.get(member)
            );
        }

        // `updateTime` is the instant of THIS call, so it never moves backwards and
        // never predates the creation.
        let update_time = timestamp_member(&record, "updateTime");
        assert!(
            update_time >= previous_update_time,
            "case {label:?}: updateTime must not move backwards, got {update_time} after \
             {previous_update_time}"
        );
        assert!(
            update_time >= timestamp_member(&created_record, "createTime"),
            "case {label:?}: updateTime must not predate createTime, got {record:?}"
        );
        previous_update_time = update_time;

        // The update PERSISTED: the read answers the record the update answered, and
        // reading it again changes nothing.
        let fetched = emulator.get_webhook(&webhook_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetWebhook, got {fetched:?}"
        );
        assert_eq!(
            fetched.member("webhook"),
            record,
            "case {label:?}: GetWebhook must answer the updated record field-for-field, got {:?}",
            fetched.member("webhook")
        );
        assert_eq!(
            emulator.get_webhook(&webhook_id).await.member("webhook"),
            record,
            "case {label:?}: reading an updated webhook must not change what a later caller sees"
        );
        assert_eq!(
            emulator.get_webhook(&bystander_id).await.member("webhook"),
            bystander_record,
            "case {label:?}: updating a webhook must not touch another app's webhook"
        );
    }
}

#[tokio::test]
async fn an_update_reaches_the_webhook_the_listing_reports() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("webhook-update-listing").await;

    // Several webhooks on one app: each update must reach exactly the one its id names
    // and leave its siblings as they were.
    let mut ids = Vec::new();
    for branch in ["main", "develop", "release"] {
        let created = emulator
            .create_webhook(
                &app_id,
                &json!({ "branchName": branch, "description": format!("{branch} trigger") }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {branch:?}: expected HTTP 200 from CreateWebhook, got {created:?}"
        );
        ids.push(string_member(&created.member("webhook"), "webhookId"));
    }

    let target = ids[1].clone();
    let updated = emulator
        .update_webhook(&target, &json!({ "description": "the only one re-described" }))
        .await;
    assert_eq!(
        updated.status, 200,
        "expected HTTP 200 from UpdateWebhook, got {updated:?}"
    );

    let listed = emulator.list_webhooks(&app_id, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListWebhooks, got {listed:?}"
    );
    let webhooks = listed.member("webhooks");
    let reported = webhooks
        .as_array()
        .expect("ListWebhooks answers a list of webhooks");
    assert_eq!(
        reported.len(),
        ids.len(),
        "an update creates no webhook and removes none, got {webhooks:?}"
    );
    for record in reported {
        let id = string_member(record, "webhookId");
        let expected = if id == target {
            "the only one re-described".to_string()
        } else {
            format!("{} trigger", string_member(record, "branchName"))
        };
        assert_eq!(
            string_member(record, "description"),
            expected,
            "case {id:?}: the listing reports the stored description, got {record:?}"
        );
        assert_eq!(
            emulator.get_webhook(&id).await.member("webhook"),
            *record,
            "case {id:?}: the listing and the read report the same stored record"
        );
    }
}

#[tokio::test]
async fn an_update_of_an_absent_webhook_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("webhook-update-refusal").await;
    let created = emulator
        .create_webhook(
            &app_id,
            &json!({ "branchName": "main", "description": "untouched by the refusals" }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateWebhook, got {created:?}"
    );
    let webhook_id = string_member(&created.member("webhook"), "webhookId");
    let created_record = created.member("webhook");

    // `WebhookId` is modelled `length 0..255, pattern ^(?s)`, so each of these passes
    // the boundary validator and the refusal is the handler's: an id no webhook has,
    // whatever its shape.
    for absent_id in [
        "no-such-webhook",
        "00000000-0000-4000-8000-000000000000",
        &"w".repeat(255),
        "App",
    ] {
        let refused = emulator
            .update_webhook(absent_id, &json!({ "description": "never stored" }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {absent_id:?}: expected HTTP 404 from UpdateWebhook, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("NotFoundException"),
            "case {absent_id:?}: expected NotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            Some(format!("Webhook {absent_id} not found.")),
            "case {absent_id:?}: the 404 names the entity the request addressed, got {:?}",
            refused.message()
        );
        assert_eq!(
            emulator.get_webhook(absent_id).await.status,
            404,
            "case {absent_id:?}: a refused update must not create the webhook it named"
        );
    }

    assert_eq!(
        emulator.get_webhook(&webhook_id).await.member("webhook"),
        created_record,
        "the refusals must leave the webhook that does exist exactly as it was"
    );
}
