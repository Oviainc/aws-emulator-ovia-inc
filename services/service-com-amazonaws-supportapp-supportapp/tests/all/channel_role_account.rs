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
//!
//! Two further tables pin WHERE the account is read from: a value whose account segment
//! is foreign is refused even when it mentions the caller's account id inside the role
//! name, and a value whose account segment is the caller's is admitted even when the
//! role name reads like another account's id or ARN. A third varies the rest of the
//! create request — the severity level, which notification flags it carries, whether it
//! names a channel — because none of that bears on which account a passed role may be
//! in.
//!
//! A value OUTSIDE the member's own `@pattern` is the same refusal, for the same reason: see
//! [`crate::channel_role_pass_role_form`], which spans those forms.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::CROSS_ACCOUNT_PASS_ROLE_MESSAGE;

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

/// `account` with its FIRST digit changed, so the result is still twelve digits and is
/// never `account` itself.
fn first_digit_changed(account: &str) -> String {
    let mut digits = account.to_string();
    let first = digits.remove(0);
    let shifted = if first == '9' { '8' } else { '9' };
    format!("{shifted}{digits}")
}

/// Foreign-account role ARNs that MENTION the caller's account id somewhere other than
/// the ARN's account segment, plus one more foreign account-id form. What settles the
/// refusal is the account the ARN NAMES, so none of these may be admitted on the
/// strength of the digits appearing in the value.
fn foreign_role_arns_mentioning_the_callers_account() -> Vec<(&'static str, String)> {
    let caller = caller_account();
    vec![
        (
            "the caller's account with its first digit changed",
            role_arn_in(&first_digit_changed(caller), "SupportAppRole"),
        ),
        (
            "a foreign account, the caller's account id inside the role name",
            role_arn_in("999999999999", &format!("{caller}-SupportAppRole")),
        ),
        (
            "a foreign account, a whole caller-account role ARN inside the role name",
            role_arn_in("999999999999", &role_arn("Nested")),
        ),
    ]
}

/// Role ARNs of the caller's OWN account whose ROLE NAME mentions another account id.
/// The account the ARN names is the caller's, so each is a role this account can pass —
/// the role name is opaque to the rule.
fn own_role_arns_mentioning_another_account() -> Vec<(&'static str, String)> {
    vec![
        (
            "a role name that is a foreign account id",
            role_arn("999999999999"),
        ),
        (
            "a role name embedding a whole foreign role ARN",
            role_arn("arn:aws:iam::999999999999:role/Other"),
        ),
        (
            "a role under a path named for a foreign account",
            role_arn("service-role/999999999999/Support"),
        ),
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

/// One create body per value `NotificationSeverityLevel` admits — `all` | `high` |
/// `none` (`smithy_query(query="shape",shape="NotificationSeverityLevel")`) — each
/// satisfying the documented severity/flag rule and each differing in which flags it
/// carries and whether it names a channel. The role's account is settled whatever else
/// the request says, so every one of these bodies must answer the same way.
fn create_bodies_across_severities(role_arn_value: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "severity none, no flags, no channel name",
            json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn_value,
            }),
        ),
        (
            "severity all, resolve flag only, named channel",
            json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "channelName": "aws-support-all",
                "notifyOnCaseSeverity": "all",
                "notifyOnResolveCase": true,
                "channelRoleArn": role_arn_value,
            }),
        ),
        (
            "severity high, correspondence flag only, no channel name",
            json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "notifyOnCaseSeverity": "high",
                "notifyOnAddCorrespondenceToCase": true,
                "channelRoleArn": role_arn_value,
            }),
        ),
    ]
}

/// Require `reply` to be the modelled pass-role refusal, worded as AWS words that refusal.
///
/// The sentence is fixed text — it quotes no value of the request back — so the row's
/// submitted value is named in the assertion's own case label instead, and the whole message
/// is pinned rather than searched for fragments.
fn assert_pass_role_refusal(case: &str, reply: &Reply, submitted: &str) {
    assert_eq!(
        reply.status, 403,
        "case {case:?} ({submitted:?}): expected HTTP 403, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "AccessDeniedException",
        "case {case:?} ({submitted:?}): expected AccessDeniedException, got {:?}",
        reply.error_type
    );
    let message = reply.string("message").unwrap_or_else(|| {
        panic!(
            "case {case:?} ({submitted:?}): the refusal carries no message: {}",
            reply.body
        )
    });
    assert_eq!(
        message, CROSS_ACCOUNT_PASS_ROLE_MESSAGE,
        "case {case:?} ({submitted:?}): expected {CROSS_ACCOUNT_PASS_ROLE_MESSAGE:?}, got \
         {message:?}"
    );
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

#[tokio::test]
async fn the_create_refusal_is_keyed_to_the_account_the_arn_names() {
    // Which account the value NAMES is the ARN's account segment, so a value that
    // mentions the caller's account id anywhere else is still another account's role,
    // and a foreign id that differs from the caller's only in its leading digit is one
    // too.
    for (case, submitted) in foreign_role_arns_mentioning_the_callers_account() {
        let emulator = Emulator::new();
        let reply = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_pass_role_refusal(case, &reply, &submitted);

        let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
        assert!(
            listed.is_none(),
            "case {case:?}: expected no configuration to be recorded, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_role_name_that_mentions_another_account_is_still_the_callers_role() {
    // The converse: the role NAME is opaque to this rule, so a role of the caller's own
    // account is created and reported verbatim even when its name reads like another
    // account's id or ARN.
    for (case, submitted) in own_role_arns_mentioning_another_account() {
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
    }
}

#[tokio::test]
async fn the_create_refusal_does_not_depend_on_the_rest_of_the_request() {
    let submitted = role_arn_in("999999999999", "SomeoneElsesRole");
    for (case, body) in create_bodies_across_severities(&submitted) {
        let emulator = Emulator::new();
        let reply = emulator.create_in_registered_workspace(body).await;
        assert_pass_role_refusal(case, &reply, &submitted);

        let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
        assert!(
            listed.is_none(),
            "case {case:?}: expected no configuration to be recorded, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_create_leaves_the_account_exactly_as_it_was() {
    // A create the pass-role rule refuses writes nothing, so the account it was aimed
    // at is unchanged: the pair it named is still free to configure, and the refusals
    // did not count against the documented channel quota.
    let emulator = Emulator::new();
    for (case, submitted) in foreign_role_arns() {
        let reply = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_pass_role_refusal(case, &reply, &submitted);
    }
    let listing = emulator.list(None).await;
    assert!(
        listing.configurations().is_empty(),
        "expected the refused creates to record nothing, got {}",
        listing.body
    );

    let own = role_arn("SupportAppRole");
    let created = emulator
        .create_in_registered_workspace(create_body(&own))
        .await;
    assert_eq!(
        created.status, 200,
        "expected the pair the refused creates named to still be free, got {} / {}",
        created.status, created.body
    );

    // And once the pair IS configured, a create carrying another account's role is
    // still the pass-role refusal — a fact about the request, settled before the
    // already-configured conflict — and the stored record is untouched.
    let submitted = role_arn_in("999999999999", "SomeoneElsesRole");
    let reply = emulator.create(create_body(&submitted)).await;
    assert_pass_role_refusal("the pair is already configured", &reply, &submitted);
    let listed = emulator
        .listed(TEAM_ID, CHANNEL_ID)
        .await
        .expect("expected the configuration to survive");
    assert_eq!(
        listed.get("channelRoleArn").and_then(Value::as_str),
        Some(own.as_str()),
        "expected the stored role ARN to be unchanged, got {listed}"
    );
}
