// SPDX-License-Identifier: CC0-1.0

//! The account a caller-supplied `channelRoleArn` must name, over the wire.
//!
//! `roleArn`'s `@pattern` (`^arn:aws:iam::[0-9]{12}:role/(.+)$`) admits ANY twelve-digit
//! account id, so a well-formed value naming SOMEONE ELSE's account reaches the handler.
//! The Support App uses that role to act for the CALLING account, so an ARN naming any
//! other account cannot be passed to it: every operation that takes the member refuses
//! such a request with the modelled `AccessDeniedException` (HTTP 403) and writes
//! nothing — on the create that first records the value and on the update that would
//! overwrite an already-validated one.
//!
//! The tables below are the rule, not one recorded request: the refused rows differ in
//! which foreign account the ARN names (one digit off the caller's, a wholly different
//! id, the all-nines id) and in the role name's form (a plain name, a name under a
//! path), and the admitted rows differ the same way inside the caller's own account.

use serde_json::{json, Value};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// The account this emulator presents, which is the only one a passed role may name
/// (`engine::EMULATOR_ACCOUNT_ID`).
fn caller_account() -> &'static str {
    engine::EMULATOR_ACCOUNT_ID
}

/// A well-formed `channelRoleArn` in `account`, which `roleArn`'s `@pattern` admits
/// whatever the twelve digits are.
fn role_arn_in(account: &str, role_name: &str) -> String {
    format!("arn:aws:iam::{account}:role/{role_name}")
}

/// The twelve-digit account ids a passed role must NOT name, each with a role-name form:
/// the caller's own id with its last digit changed, a wholly different id, and the
/// all-nines id. Every value satisfies `^arn:aws:iam::[0-9]{12}:role/(.+)$`, so each one
/// reaches the handler.
fn foreign_role_arns() -> Vec<(&'static str, String)> {
    let caller = caller_account();
    let one_digit_off = {
        let mut digits = caller.to_string();
        let last = digits.pop().expect("a twelve-digit account id");
        let shifted = if last == '9' { '8' } else { '9' };
        digits.push(shifted);
        digits
    };
    vec![
        (
            "the caller's account with one digit changed",
            role_arn_in(&one_digit_off, "SupportAppRole"),
        ),
        (
            "a wholly different account id",
            role_arn_in("123456789012", "SupportAppRole"),
        ),
        (
            "the all-nines account id",
            role_arn_in("999999999999", "SomeoneElsesRole"),
        ),
        (
            "another account, role under a path",
            role_arn_in("210987654321", "service-role/path/SupportAppRole"),
        ),
        (
            "another account, longest role name the member admits",
            // `channelRoleArn` is `@length min 31 max 2048`, so the longest value the
            // member admits is the ARN prefix padded out to 2048 characters.
            role_arn_in(
                "555555555555",
                &"R".repeat(2048 - role_arn_in("555555555555", "").len()),
            ),
        ),
    ]
}

/// The role ARNs of the caller's OWN account that must be admitted, so the refusal is
/// keyed to the account and not to the member being present at all.
fn own_role_arns() -> Vec<(&'static str, String)> {
    vec![
        ("a plain role name", role_arn("SupportAppRole")),
        ("a role under a path", role_arn("service-role/Support")),
        ("a single-character role name", role_arn("r")),
    ]
}

/// The `(teamId, channelId)` every row configures, and the role the configuration is
/// created with.
const TEAM_ID: &str = "T012ABCDEFG";
const CHANNEL_ID: &str = "C01234A5BCD";

fn create_body(role_arn_value: &str) -> Value {
    json!({
        "teamId": TEAM_ID,
        "channelId": CHANNEL_ID,
        "channelName": "aws-support",
        "notifyOnCaseSeverity": "high",
        "notifyOnCreateOrReopenCase": true,
        "channelRoleArn": role_arn_value,
    })
}

/// Require `reply` to be the modelled pass-role refusal, and that its message states
/// the rule: which parameter was refused, which value was submitted, and which account
/// a passed role may name.
fn assert_pass_role_refusal(case: &str, reply: &Reply, submitted: &str) {
    assert_eq!(
        reply.status, 403,
        "case {case:?}: expected HTTP 403, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {:?}",
        reply.error_type
    );
    let message = reply.string("message").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the refusal carries no message: {}",
            reply.body
        )
    });
    for expected in ["ChannelRoleArn", submitted, caller_account()] {
        assert!(
            message.contains(expected),
            "case {case:?}: expected the message to state {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn creating_a_configuration_with_another_accounts_role_is_refused() {
    for (case, submitted) in foreign_role_arns() {
        let emulator = Emulator::new();
        let reply = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_pass_role_refusal(case, &reply, &submitted);

        // And the refused create recorded nothing: there is no configuration to read.
        let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
        assert!(
            listed.is_none(),
            "case {case:?}: expected no configuration to be recorded, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn updating_a_configuration_onto_another_accounts_role_is_refused() {
    for (case, submitted) in foreign_role_arns() {
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(create_body(&role_arn("SupportAppRole")))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the fixture create to succeed, got {} / {}",
            created.status, created.body
        );

        let reply = emulator
            .update(json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "channelRoleArn": submitted,
            }))
            .await;
        assert_pass_role_refusal(case, &reply, &submitted);

        // The refused update left the already-validated record exactly as it stood —
        // including every member it did not name.
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to survive"));
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(role_arn("SupportAppRole").as_str()),
            "case {case:?}: expected the stored role ARN to be unchanged, got {listed}"
        );
        assert_eq!(
            listed.get("channelName").and_then(Value::as_str),
            Some("aws-support"),
            "case {case:?}: expected the rest of the record to be unchanged, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_refusal_is_about_the_request_not_about_the_stored_configuration() {
    // The account a passed role names is a fact about the REQUEST, so an update that
    // carries a foreign role is refused for that reason whether or not the
    // `(teamId, channelId)` pair names a configuration this account holds.
    let emulator = Emulator::new();
    let submitted = role_arn_in("999999999999", "SomeoneElsesRole");
    let reply = emulator
        .update(json!({
            "teamId": TEAM_ID,
            "channelId": CHANNEL_ID,
            "channelRoleArn": submitted,
        }))
        .await;
    assert_pass_role_refusal("no such configuration", &reply, &submitted);

    // Same precedence on the create path: the role's account is settled before the
    // workspace registry is consulted, so a create whose workspace is not registered
    // AND whose role is another account's is the pass-role refusal, not the
    // unregistered-workspace conflict.
    let reply = emulator.create(create_body(&submitted)).await;
    assert_pass_role_refusal("no registered workspace either", &reply, &submitted);
}

#[tokio::test]
async fn a_role_in_the_callers_own_account_is_admitted_on_both_writes() {
    for (case, submitted) in own_role_arns() {
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to be recorded"));
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(submitted.as_str()),
            "case {case:?}: expected the created role ARN to be reported, got {listed}"
        );

        // The same value on the update path, re-pointed at another role of the SAME
        // account: accepted, answered and persisted.
        let repointed = role_arn("RepointedRole");
        let updated = emulator
            .update(json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "channelRoleArn": repointed,
            }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected update to answer HTTP 200, got {} / {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("channelRoleArn"),
            Some(repointed.clone()),
            "case {case:?}: expected the update to answer with the new role ARN, got {}",
            updated.body
        );
        let listed = emulator
            .listed(TEAM_ID, CHANNEL_ID)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to survive"));
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(repointed.as_str()),
            "case {case:?}: expected the updated role ARN to be persisted, got {listed}"
        );
    }
}
