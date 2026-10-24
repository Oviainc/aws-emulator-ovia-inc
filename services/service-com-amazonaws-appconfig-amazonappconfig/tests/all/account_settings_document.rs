// SPDX-License-Identifier: CC0-1.0

//! The shape of the account-settings document: `GET /settings` and `PATCH /settings`
//! both answer the model's `AccountSettings`, and that document always carries EVERY
//! settings group it declares.
//!
//! The rule under test: every settings-group member of the account-settings document is
//! reported on every account-settings response — the group goes on the wire even when
//! the account holds no written value inside it (then it is the empty document `{}`),
//! and only the unset members INSIDE a group are omitted. An account always has a
//! deletion-protection and a vended-metrics configuration, so neither group can be
//! absent; `VendedMetricsSettings$Enabled` has no modelled default, so that member is
//! absent until it is written rather than invented.
//!
//! Rows span what the rule generalises over: which groups a request names, which nested
//! member it names, the value it writes, the order the writes arrive in, and how many
//! writes the account has seen (including none at all).

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{json_call, router, Answer};

/// The settings groups the `AccountSettings` document declares, from the model:
/// `smithy_query(query="shape",shape="com.amazonaws.appconfig#AccountSettings")` reports
/// members `DeletionProtection` (-> `DeletionProtectionSettings`) and `VendedMetrics`
/// (-> `VendedMetricsSettings`), neither of them `@required`.
const SETTINGS_GROUPS: &[&str] = &["DeletionProtection", "VendedMetrics"];

/// `GET /settings`.
async fn get_settings(router: &MultiServiceRouter) -> Answer {
    json_call(router, "GET", "/settings", &Value::Null).await
}

/// `PATCH /settings`.
async fn patch_settings(router: &MultiServiceRouter, body: &Value) -> Answer {
    json_call(router, "PATCH", "/settings", body).await
}

/// Assert one account-settings response answers 200 and carries every settings group as
/// a JSON object. `case` names the row and `call` the operation that answered.
fn assert_document(case: &str, call: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: {call} must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    for group in SETTINGS_GROUPS {
        let reported = body.get(group);
        assert!(
            reported.is_some_and(Value::is_object),
            "case {case:?}: {call} must report the {group:?} settings group as an object, \
             got {reported:?} in {body}"
        );
    }
}

/// Every account-settings response carries every settings group, whatever the account
/// has been written — including an account nothing has ever written.
#[tokio::test]
async fn every_account_settings_response_carries_every_settings_group() {
    /// One row: the sequence of patches an account has seen, applied in order.
    struct Row {
        case: &'static str,
        patches: Vec<Value>,
    }
    let rows = vec![
        Row {
            case: "an account nothing has ever written",
            patches: vec![],
        },
        Row {
            case: "an empty patch",
            patches: vec![json!({})],
        },
        Row {
            case: "deletion protection only",
            patches: vec![json!({ "DeletionProtection": { "Enabled": true } })],
        },
        Row {
            case: "a nested member of deletion protection only",
            patches: vec![json!({ "DeletionProtection": { "ProtectionPeriodInMinutes": 15 } })],
        },
        Row {
            case: "vended metrics only, enabled",
            patches: vec![json!({ "VendedMetrics": { "Enabled": true } })],
        },
        Row {
            case: "vended metrics only, disabled",
            patches: vec![json!({ "VendedMetrics": { "Enabled": false } })],
        },
        Row {
            case: "an empty vended-metrics group writes nothing inside it",
            patches: vec![json!({ "VendedMetrics": {} })],
        },
        Row {
            case: "both groups at once",
            patches: vec![json!({
                "DeletionProtection": { "Enabled": false, "ProtectionPeriodInMinutes": 1440 },
                "VendedMetrics": { "Enabled": true }
            })],
        },
        Row {
            case: "vended metrics, then deletion protection",
            patches: vec![
                json!({ "VendedMetrics": { "Enabled": true } }),
                json!({ "DeletionProtection": { "Enabled": true } }),
            ],
        },
        Row {
            case: "deletion protection, then vended metrics",
            patches: vec![
                json!({ "DeletionProtection": { "Enabled": true } }),
                json!({ "VendedMetrics": { "Enabled": false } }),
            ],
        },
    ];

    for Row { case, patches } in &rows {
        // A fresh account per row, so a row's document depends only on its own patches.
        let router = router();
        for patch in patches {
            assert_document(case, "UpdateAccountSettings", &patch_settings(&router, patch).await);
        }
        assert_document(case, "GetAccountSettings", &get_settings(&router).await);
    }
}

/// A settings group the account holds no written value inside is the EMPTY document, not
/// an absent group and not a fabricated value: `VendedMetrics` reports `{}` until
/// `Enabled` is written, and reports what was written from then on.
#[tokio::test]
async fn an_unwritten_settings_group_is_reported_as_the_empty_document() {
    // Rows differ along the patch that precedes the read and along what it leaves the
    // vended-metrics group holding: nothing at all, or each of the member's two values.
    let rows: Vec<(&str, Option<Value>, Value)> = vec![
        ("nothing written", None, json!({})),
        ("an empty patch", Some(json!({})), json!({})),
        (
            "a patch naming another group",
            Some(json!({ "DeletionProtection": { "Enabled": true } })),
            json!({}),
        ),
        (
            "an empty vended-metrics group",
            Some(json!({ "VendedMetrics": {} })),
            json!({}),
        ),
        (
            "vended metrics enabled",
            Some(json!({ "VendedMetrics": { "Enabled": true } })),
            json!({ "Enabled": true }),
        ),
        (
            "vended metrics disabled",
            Some(json!({ "VendedMetrics": { "Enabled": false } })),
            json!({ "Enabled": false }),
        ),
    ];

    for (case, patch, expected) in &rows {
        let router = router();
        if let Some(patch) = patch {
            let updated = patch_settings(&router, patch).await;
            assert_eq!(
                updated.json().get("VendedMetrics"),
                Some(expected),
                "case {case:?}: UpdateAccountSettings must report the vended-metrics group it \
                 stored, got {}",
                updated.json()
            );
        }
        let read = get_settings(&router).await;
        assert_eq!(
            read.json().get("VendedMetrics"),
            Some(expected),
            "case {case:?}: GetAccountSettings must report the vended-metrics group, got {}",
            read.json()
        );
    }
}

/// The document reports no settings group the model does not declare, so "always report
/// every group" cannot be satisfied by reporting extra ones.
#[tokio::test]
async fn the_account_settings_document_reports_only_the_modelled_groups() {
    let router = router();
    // Rows differ along how much of the document has been written, since a stray member
    // could come from either handler.
    let patches = vec![
        json!({}),
        json!({ "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 720 } }),
        json!({ "VendedMetrics": { "Enabled": true } }),
    ];
    let mut answers: Vec<(&str, Answer)> = Vec::new();
    for patch in &patches {
        answers.push(("UpdateAccountSettings", patch_settings(&router, patch).await));
        answers.push(("GetAccountSettings", get_settings(&router).await));
    }
    for (call, answer) in &answers {
        let body = answer.json();
        let members: &Map<String, Value> = body
            .as_object()
            .unwrap_or_else(|| panic!("case {call:?}: expected a JSON object, got {body}"));
        for member in members.keys() {
            assert!(
                SETTINGS_GROUPS.contains(&member.as_str()),
                "case {call:?}: {member:?} is not a modelled AccountSettings member, in {body}"
            );
        }
    }
}
