// SPDX-License-Identifier: CC0-1.0

//! WHICH LAYER judges the FORM of a `channelRoleArn`, over the wire.
//!
//! THE RULE UNDER TEST: `channelRoleArn` names a role the caller asks the Support App to
//! USE ("The Amazon Resource Name (ARN) of an IAM role that you want to use to perform
//! operations on Amazon Web Services",
//! `smithy_query(query="doc",shape="CreateSlackChannelConfigurationRequest",member="channelRoleArn")`),
//! so the value is judged by the pass-role step rather than by the request boundary: a value
//! that does not name an IAM role of the CALLING account is refused with the modelled
//! `AccessDeniedException` (HTTP 403) and AWS's wording for that refusal — whatever form it
//! has, the member's own `@pattern` and `@length` included — and nothing is recorded. A
//! caller-input refusal (400 `ValidationException`) is NOT the answer for such a value: the
//! two tell a request bug apart from a role or trust-policy problem, and a client branches on
//! which one it got.
//!
//! The tables are that rule, not one recorded request. The refused rows span HOW a value
//! leaves the member's form — no ARN at all, an ARN of another service, an IAM entity that is
//! not a role, an account segment that is not twelve digits, an empty account segment, a
//! value below the published `@length` minimum, a role ARN of this account with no role name
//! after `role/`, and this account's own role ARN with its ARN scheme upper-cased — and are
//! driven on BOTH operations that take the member. The admitted rows are the boundary of the
//! rule: a role of the caller's own account in several forms is served.
//!
//! Two rules of the member are NOT the pass-role step's, and each has a test here, because
//! they are what keeps the rule from being read as "this member is no longer checked": the
//! member stays `@required`, so a request carrying none at all is a 400; and its declared
//! `@length` maximum still bounds what this API can carry, so a value past it is a 400 too —
//! a caller-input mistake rather than a role the service may not pass.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    CROSS_ACCOUNT_PASS_ROLE_MESSAGE, MAX_CHANNEL_ROLE_ARN_LEN,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// The `(teamId, channelId)` every row below configures.
const TEAM_ID: &str = "T012ABCDEFG";
const CHANNEL_ID: &str = "C01234A5BCD";

/// The sentence AWS answers a refused pass-role attempt with, written out here so the
/// WORDING is pinned by this suite rather than read back off the code under test: every
/// assertion below compares the response against this literal, and one case pins the crate's
/// own exported constant to it as well.
const AWS_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// `channelRoleArn` is `@length min 31 max 2048`
/// (`smithy_query(query="operation",operation="CreateSlackChannelConfiguration")`), so a
/// 30-character value is one below the published minimum and a 2049-character one is one past
/// the published maximum — the bound the crate states from the same member.
const ROLE_ARN_MIN_LEN: usize = 31;
const ROLE_ARN_MAX_LEN: usize = MAX_CHANNEL_ROLE_ARN_LEN;

/// Values that name no IAM role of the calling account, each leaving the member's modelled
/// form in a different way. Every one of them is a pass-role refusal.
fn values_naming_no_role_of_this_account() -> Vec<(&'static str, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        // The recorded case's class: a value that is no ARN at all, padded to the member's
        // published length minimum so nothing but its form is wrong.
        (
            "no ARN at all",
            format!("not-an-iam-role-arn{}", "0".repeat(ROLE_ARN_MIN_LEN)),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:sts::{account}:assumed-role/SupportAppRole/session"),
        ),
        (
            "an IAM entity that is not a role",
            format!("arn:aws:iam::{account}:user/SupportAppRole"),
        ),
        (
            "an account segment that is not twelve digits",
            "arn:aws:iam::ABCDEFGHIJKL:role/SupportAppRole".to_string(),
        ),
        (
            "an empty account segment",
            "arn:aws:iam:::role/SupportAppRoleWithALongEnoughName".to_string(),
        ),
        (
            "one character below the published length minimum",
            role_arn("")[..ROLE_ARN_MIN_LEN - 1].to_string(),
        ),
        (
            "a foreign account's role ARN at the published length maximum",
            format!(
                "arn:aws:iam::999999999999:role/{}",
                "R".repeat(ROLE_ARN_MAX_LEN - "arn:aws:iam::999999999999:role/".len())
            ),
        ),
        (
            "this account's role ARN with no role name after it",
            role_arn(""),
        ),
        (
            "this account's role ARN with its ARN scheme upper-cased",
            format!("ARN:AWS:IAM::{account}:role/SupportAppRole"),
        ),
    ]
}

/// Roles of the caller's OWN account, in several name forms: the boundary of the rule, each
/// one served.
fn roles_of_this_account() -> Vec<(&'static str, String)> {
    vec![
        ("a plain role name", role_arn("SupportAppRole")),
        ("a role under a path", role_arn("service-role/Support")),
        ("a single-character role name", role_arn("r")),
        (
            "a role name at the published length maximum",
            role_arn(&"R".repeat(ROLE_ARN_MAX_LEN - role_arn("").len())),
        ),
    ]
}

/// A create body carrying `channel_role_arn`, with everything else the model requires.
fn create_body(channel_role_arn: &str) -> Value {
    json!({
        "teamId": TEAM_ID,
        "channelId": CHANNEL_ID,
        "channelName": "aws-support",
        "notifyOnCaseSeverity": "none",
        "channelRoleArn": channel_role_arn,
    })
}

/// Require `reply` to be the pass-role refusal: the modelled `AccessDeniedException`, HTTP
/// 403, worded as AWS words that refusal.
fn assert_pass_role_refusal(case: &str, submitted: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 403,
        "case {case:?} ({submitted:?}): expected HTTP 403, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "AccessDeniedException",
        "case {case:?} ({submitted:?}): expected AccessDeniedException, got {:?} / {}",
        reply.error_type, reply.body
    );
    let message = reply.string("message").unwrap_or_else(|| {
        panic!(
            "case {case:?} ({submitted:?}): the refusal carries no message: {}",
            reply.body
        )
    });
    assert_eq!(
        message, AWS_PASS_ROLE_MESSAGE,
        "case {case:?} ({submitted:?}): expected {AWS_PASS_ROLE_MESSAGE:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_create_naming_no_role_of_this_account_is_the_pass_role_refusal() {
    // The wording the crate carries for this refusal IS the sentence AWS answers with, so a
    // drift in the constant fails here rather than passing unnoticed through the rows below.
    assert_eq!(
        CROSS_ACCOUNT_PASS_ROLE_MESSAGE, AWS_PASS_ROLE_MESSAGE,
        "case \"the refusal's wording\": expected {AWS_PASS_ROLE_MESSAGE:?}, got \
         {CROSS_ACCOUNT_PASS_ROLE_MESSAGE:?}"
    );
    for (case, submitted) in values_naming_no_role_of_this_account() {
        let emulator = Emulator::new();
        let reply = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_pass_role_refusal(case, &submitted, &reply);

        // The refused create recorded nothing: the pair it named is still unconfigured.
        let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
        assert!(
            listed.is_none(),
            "case {case:?} ({submitted:?}): expected no configuration to be recorded, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_update_onto_no_role_of_this_account_is_the_pass_role_refusal() {
    for (case, submitted) in values_naming_no_role_of_this_account() {
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(create_body(&role_arn("SupportAppRole")))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the fixture create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );

        let reply = emulator
            .update(json!({
                "teamId": TEAM_ID,
                "channelId": CHANNEL_ID,
                "channelRoleArn": submitted,
            }))
            .await;
        assert_pass_role_refusal(case, &submitted, &reply);

        // The refused update left the stored role exactly as the create recorded it.
        let listed = reported(&emulator, case).await;
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(role_arn("SupportAppRole").as_str()),
            "case {case:?} ({submitted:?}): expected the stored role ARN to be unchanged, got \
             {listed}"
        );
    }
}

#[tokio::test]
async fn a_role_of_this_account_is_served_on_both_writes() {
    for (case, submitted) in roles_of_this_account() {
        let emulator = Emulator::new();
        let created = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected create to answer HTTP 200, got {} / {}",
            created.status, created.body
        );
        let listed = reported(&emulator, case).await;
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(submitted.as_str()),
            "case {case:?}: expected the created role ARN to be reported, got {listed}"
        );

        // And the same value on the update path, re-pointed at another role of the same
        // account: served, answered and persisted.
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
        let listed = reported(&emulator, case).await;
        assert_eq!(
            listed.get("channelRoleArn").and_then(Value::as_str),
            Some(repointed.as_str()),
            "case {case:?}: expected the updated role ARN to be persisted, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_member_is_still_required() {
    // The pass-role step judges the value a request CARRIES. The member is `@required`, so a
    // create that carries none at all is the boundary's own request-validation refusal, and
    // records nothing.
    let emulator = Emulator::new();
    let mut body = create_body(&role_arn("SupportAppRole"));
    body.as_object_mut()
        .expect("a create body")
        .remove("channelRoleArn");
    let reply = emulator.create_in_registered_workspace(body).await;
    assert_eq!(
        reply.status, 400,
        "case \"channelRoleArn absent\": expected HTTP 400, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ValidationException",
        "case \"channelRoleArn absent\": expected ValidationException, got {:?} / {}",
        reply.error_type, reply.body
    );
    let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
    assert!(
        listed.is_none(),
        "case \"channelRoleArn absent\": expected no configuration to be recorded, got {listed:?}"
    );
}

/// The configuration the rows above write, read back through the listing.
async fn reported(emulator: &Emulator, case: &str) -> Value {
    emulator
        .listed(TEAM_ID, CHANNEL_ID)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: expected the configuration to be reported"))
}

#[tokio::test]
async fn a_value_past_the_published_length_maximum_is_a_request_validation_refusal() {
    // The member's declared `@length` maximum bounds what this API can carry, so a value past
    // it is a caller-input mistake — 400 `ValidationException`, in the SAME standard validation
    // envelope the request boundary states that constraint class in for every other member of
    // this service — and not the pass-role refusal, even for a role of the caller's own
    // account. The rows differ in whose account the over-long value names, because the bound is
    // about the value's size and not about the role.
    let one_past = ROLE_ARN_MAX_LEN + 1;
    for (case, submitted) in [
        (
            "this account's role, one character past the maximum",
            role_arn(&"R".repeat(one_past - role_arn("").len())),
        ),
        (
            "a foreign account's role, one character past the maximum",
            format!(
                "arn:aws:iam::999999999999:role/{}",
                "R".repeat(one_past - "arn:aws:iam::999999999999:role/".len())
            ),
        ),
    ] {
        assert_eq!(
            submitted.chars().count(),
            one_past,
            "case {case:?}: the row must be one character past the maximum"
        );
        let emulator = Emulator::new();
        let reply = emulator
            .create_in_registered_workspace(create_body(&submitted))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?} / {}",
            reply.error_type, reply.body
        );
        let message = reply
            .string("message")
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message"));
        // The clause AWS states a `@length` maximum with, written out: the value submitted,
        // the member path in its lowerCamel spelling, and the bound the member declares.
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at 'channelRoleArn' failed to \
             satisfy constraint: Member must have length less than or equal to \
             {MAX_CHANNEL_ROLE_ARN_LEN}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        let listed = emulator.listed(TEAM_ID, CHANNEL_ID).await;
        assert!(
            listed.is_none(),
            "case {case:?}: expected no configuration to be recorded, got {listed:?}"
        );
    }

    // And the same value one character SHORTER — exactly at the published maximum — is
    // served, so the bound is the member's and not an invented one.
    let emulator = Emulator::new();
    let at_the_maximum = role_arn(&"R".repeat(ROLE_ARN_MAX_LEN - role_arn("").len()));
    let created = emulator
        .create_in_registered_workspace(create_body(&at_the_maximum))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"at the published maximum\": expected HTTP 200, got {} / {}",
        created.status, created.body
    );
    let listed = reported(&emulator, "at the published maximum").await;
    assert_eq!(
        listed.get("channelRoleArn").and_then(Value::as_str),
        Some(at_the_maximum.as_str()),
        "case \"at the published maximum\": expected the stored role ARN to be reported, got \
         {listed}"
    );
}
