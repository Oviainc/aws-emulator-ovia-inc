// SPDX-License-Identifier: CC0-1.0

//! RULE: a target account configuration names ONE Amazon Web Services account and the IAM
//! role FIS assumes in it, so what it may be configured with is decided by those two
//! members together: an `accountId` that is not written as an account number — a decimal
//! figure — is refused naming `accountId`, and the `roleArn` has to name a role of the
//! account the `accountId` names, which is compared for EVERY value that member can carry.
//! Both are `ValidationException` (HTTP 400) and persist nothing, on
//! `CreateTargetAccountConfiguration` and on `UpdateTargetAccountConfiguration` alike.
//!
//! `TargetAccountId` is modelled only as a string within `@length(12..48)`,
//! `pattern ^[\S]+$` and `RoleArn` within `@length(20..2048)`, `pattern ^[\S]+$`
//! (`smithy_query(query="shape",shape="TargetAccountId,RoleArn")`), so neither rule is
//! expressible in the model: the boundary admits every value below, and the server is what
//! decides it. The account field of an IAM role ARN is an account number, so the two rules
//! together admit exactly the account ids a role can belong to — a figure of any other
//! length is refused by the role it could never own.
//!
//! The rows differ along everything the rule generalises over: the shape of the
//! `accountId` (an account number, one carrying leading zeros, a figure with too many
//! digits, a figure at `TargetAccountId`'s published ceiling, a value below its published
//! floor, non-numeric text, an alias, digits broken by separators), and whether the role
//! offered for it belongs to it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{create_template, emulator, send, token, Answer, FALLBACK_REGION};

/// The account every row offers a role of: one well-formed AWS account number, so the row's
/// own `accountId` is the only thing that differs.
const ROLE_ACCOUNT: &str = "123456789012";

/// A template a configuration can be attached to: one that targets MULTIPLE accounts, which
/// is what a target account configuration belongs to.
async fn multi_account_template(router: &MultiServiceRouter, label: &str) -> String {
    let created = create_template(
        router,
        json!({
            "clientToken": token(&format!("account-id-template-{label}")),
            "description": format!("accountId rule: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-account-id-rule",
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "actions": {
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } }
            },
            "experimentOptions": { "accountTargeting": "multi-account" }
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: expected the template to be created, got {} with {:?}",
        created.status, created.body
    );
    created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no template id in {:?}", created.body))
        .to_string()
}

/// The path of one target account configuration.
fn configuration_path(template_id: &str, account_id: &str) -> String {
    format!("/experimentTemplates/{template_id}/targetAccountConfigurations/{account_id}")
}

async fn create_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
    body: Value,
) -> Answer {
    send(
        router,
        "POST",
        &configuration_path(template_id, account_id),
        Some(body),
        FALLBACK_REGION,
    )
    .await
}

async fn update_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
    body: Value,
) -> Answer {
    send(
        router,
        "PATCH",
        &configuration_path(template_id, account_id),
        Some(body),
        FALLBACK_REGION,
    )
    .await
}

async fn get_configuration(
    router: &MultiServiceRouter,
    template_id: &str,
    account_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &configuration_path(template_id, account_id),
        None,
        FALLBACK_REGION,
    )
    .await
}

/// A role of `account`, named for `label`.
fn role_arn(account: &str, label: &str) -> String {
    format!("arn:aws:iam::{account}:role/fis-{label}")
}

/// `case` as an idempotency token can carry it: `ClientToken` is modelled
/// `pattern ^[\S]+$`, so the spaces a case name reads well with are not admissible in one.
fn slug(case: &str) -> String {
    case.chars()
        .map(|character| if character.is_whitespace() { '-' } else { character })
        .collect()
}

/// One row: the `accountId` under test, the account the role offered for it belongs to, and
/// what the pair earns.
struct Row {
    case: &'static str,
    account_id: String,
    /// The account the `roleArn` belongs to, or `None` for a row that offers a role of its
    /// OWN `accountId` — the pairing the ownership comparison cannot refuse, so what
    /// decides it is whether that figure can be an account at all.
    role_account: Option<&'static str>,
    /// The HTTP status the pair earns.
    expected: u16,
    /// The member a refusal names, where the rule that refuses it is this service's own.
    /// `None` for a value the BOUNDARY refuses on the modelled length, whose wording
    /// `length_refusal_wording.rs` owns.
    names: Option<&'static str>,
}

/// The rows the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an account number and a role of it",
            account_id: ROLE_ACCOUNT.to_string(),
            role_account: Some(ROLE_ACCOUNT),
            expected: 200,
            names: None,
        },
        Row {
            case: "an account number carrying leading zeros, and a role of it",
            account_id: "000000000007".to_string(),
            role_account: Some("000000000007"),
            expected: 200,
            names: None,
        },
        Row {
            case: "an account number whose role belongs to another account",
            account_id: "999988887777".to_string(),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: Some("roleArn"),
        },
        Row {
            case: "a figure with one digit too many",
            account_id: "1234567890123".to_string(),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: Some("roleArn"),
        },
        Row {
            case: "a figure with one digit too many, offered a role of that same figure",
            account_id: "1234567890123".to_string(),
            role_account: None,
            expected: 400,
            names: Some("roleArn"),
        },
        Row {
            case: "a figure at TargetAccountId's published ceiling",
            account_id: "9".repeat(48),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: Some("roleArn"),
        },
        Row {
            case: "a figure at the published ceiling, offered a role of that same figure",
            account_id: "9".repeat(48),
            role_account: None,
            expected: 400,
            names: Some("roleArn"),
        },
        Row {
            case: "twelve characters that are no figure at all",
            account_id: "abcdefghijkl".to_string(),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: Some("accountId"),
        },
        Row {
            case: "an account alias rather than a number",
            account_id: "my-target-account".to_string(),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: Some("accountId"),
        },
        Row {
            case: "digits broken by separators",
            account_id: "1111-2222-3333".to_string(),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: Some("accountId"),
        },
        Row {
            case: "one character under TargetAccountId's published floor",
            account_id: "1".repeat(11),
            role_account: Some(ROLE_ACCOUNT),
            expected: 400,
            names: None,
        },
    ]
}

impl Row {
    /// The `roleArn` this row offers, named for `label`: a role of the account the row
    /// names, or of its own `accountId` where the row is about that pairing.
    fn offered_role(&self, label: &str) -> String {
        role_arn(self.role_account.unwrap_or(&self.account_id), label)
    }
}

#[tokio::test]
async fn a_configuration_takes_only_an_account_number_and_a_role_of_that_account() {
    let router = emulator();
    let template_id = multi_account_template(&router, "create").await;

    for row in rows() {
        let offered = row.offered_role("created");
        let answer = create_configuration(
            &router,
            &template_id,
            &row.account_id,
            json!({
                "roleArn": offered,
                "clientToken": token(&format!("account-id-create-{}", slug(row.case)))
            }),
        )
        .await;
        assert_eq!(
            answer.status, row.expected,
            "case {:?}: expected HTTP {} from CreateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, row.expected, answer.status, answer.body
        );

        if row.expected == 200 {
            // The round trip: the read reports what the create persisted.
            let fetched = get_configuration(&router, &template_id, &row.account_id).await;
            assert_eq!(
                fetched.member("targetAccountConfiguration")["roleArn"],
                json!(offered),
                "case {:?}: expected the read to report the created roleArn, got {:?}",
                row.case,
                fetched.body
            );
            continue;
        }

        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        if let Some(member) = row.names {
            assert!(
                message.contains(member),
                "case {:?}: expected the refusal to name {member:?}, got {message:?}",
                row.case
            );
        }
        // A refused create persists nothing under the name it named.
        let fetched = get_configuration(&router, &template_id, &row.account_id).await;
        assert_ne!(
            fetched.status, 200,
            "case {:?}: a refused create must persist nothing, got {:?}",
            row.case, fetched.body
        );
    }
}

#[tokio::test]
async fn an_update_takes_only_an_account_number_and_a_role_of_that_account() {
    let router = emulator();
    let template_id = multi_account_template(&router, "update").await;

    for row in rows() {
        // The record an accepted row updates, created with a role of its own account.
        let baseline = role_arn(&row.account_id, "baseline");
        if row.expected == 200 {
            let created = create_configuration(
                &router,
                &template_id,
                &row.account_id,
                json!({
                    "roleArn": baseline,
                    "clientToken": token(&format!(
                        "account-id-update-baseline-{}",
                        slug(row.case)
                    ))
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected HTTP 200 creating the record to update, got {} with {:?}",
                row.case, created.status, created.body
            );
        }

        let offered = row.offered_role("updated");
        let answer = update_configuration(
            &router,
            &template_id,
            &row.account_id,
            json!({ "roleArn": offered }),
        )
        .await;
        assert_eq!(
            answer.status, row.expected,
            "case {:?}: expected HTTP {} from UpdateTargetAccountConfiguration, got {} \
             with {:?}",
            row.case, row.expected, answer.status, answer.body
        );

        if row.expected == 200 {
            let fetched = get_configuration(&router, &template_id, &row.account_id).await;
            assert_eq!(
                fetched.member("targetAccountConfiguration")["roleArn"],
                json!(offered),
                "case {:?}: expected the update to replace the stored roleArn, got {:?}",
                row.case,
                fetched.body
            );
            continue;
        }

        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        if let Some(member) = row.names {
            assert!(
                message.contains(member),
                "case {:?}: expected the refusal to name {member:?}, got {message:?}",
                row.case
            );
        }
        // And a refused update created nothing: the name it refused holds no record.
        let fetched = get_configuration(&router, &template_id, &row.account_id).await;
        assert_ne!(
            fetched.status, 200,
            "case {:?}: a refused update must persist nothing, got {:?}",
            row.case, fetched.body
        );
    }
}
