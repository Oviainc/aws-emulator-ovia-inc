// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two account-level state rules of the Support App:
//!
//! 1. a Slack channel configuration can only exist inside a Slack workspace that is
//!    registered to the account, so `CreateSlackChannelConfiguration` refuses one for
//!    an unregistered `teamId` with the modelled `ConflictException` (409) and stores
//!    nothing;
//! 2. `DeleteAccountAlias` removes the account's alias, so a later `GetAccountAlias`
//!    reports an account with no alias — a 200 whose optional `accountAlias` member is
//!    absent — and a delete with no alias to remove is `ResourceNotFoundException`
//!    (404).
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]: only the
//! HTTP status, the `x-amzn-errortype` header and the JSON body are inspected, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The tests are tables. Their rows differ along what each rule generalises over — the
//! team and channel id form and length, the channel name, which notification flags are
//! sent and at which severity, how many workspaces are registered, and the alias
//! charset and length — so the recorded `T012ABCDEFG` / alias case is one row of many.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    derived_slack_workspace_name, ACCOUNT_TYPE, MAX_SLACK_WORKSPACE_CONFIGURATIONS,
};

use crate::slack_channel_configurations::{role_arn, Emulator};

/// The values `AccountType` accepts
/// (`smithy_query(query="shape", shape="AccountType")`). The emulator presents a
/// single account, which is therefore the management account of its own organization,
/// so `ACCOUNT_TYPE` is the value this service reports; the set is here to pin that
/// the reported value is one the model declares.
const ACCOUNT_TYPE_VALUES: [&str; 2] = ["management", "member"];

/// `accountAlias` is `len 1..30, pattern ^[\w\- ]+$`
/// (`smithy_query(query="shape", shape="awsAccountAlias")`).
const ACCOUNT_ALIAS_MIN_LEN: usize = 1;
const ACCOUNT_ALIAS_MAX_LEN: usize = 30;

/// One row of the create table: a request that satisfies every modelled constraint, so
/// the only thing that can refuse it is the workspace precondition.
#[derive(Debug, Clone)]
struct CreateCase {
    label: &'static str,
    team_id: String,
    channel_id: String,
    channel_name: Option<String>,
    severity: &'static str,
    notify_on_resolve_case: bool,
}

impl CreateCase {
    fn body(&self) -> Value {
        let mut body = json!({
            "teamId": self.team_id,
            "channelId": self.channel_id,
            "notifyOnCaseSeverity": self.severity,
            "notifyOnResolveCase": self.notify_on_resolve_case,
            "channelRoleArn": role_arn("SupportAppRole"),
        });
        if let Some(channel_name) = &self.channel_name {
            body["channelName"] = json!(channel_name);
        }
        body
    }
}

/// The create table: rows differ in team and channel id form and length (both
/// published `len 1..256` bounds are covered), whether a channel name is sent, the
/// severity level and the notification flag it requires. `none` requires every flag
/// false and `high`/`all` require at least one true, so each row is a valid request.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the recorded case",
            team_id: "T012ABCDEFG".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            channel_name: Some("aws-support".to_string()),
            severity: "none",
            notify_on_resolve_case: false,
        },
        CreateCase {
            label: "the shortest legal ids, no channel name",
            team_id: "T".to_string(),
            channel_id: "C".to_string(),
            channel_name: None,
            severity: "high",
            notify_on_resolve_case: true,
        },
        CreateCase {
            label: "mixed-case punctuated ids, severity all",
            team_id: "T-mixed.Case_9".to_string(),
            channel_id: "C-mixed.Case_9".to_string(),
            channel_name: Some("a channel name with spaces".to_string()),
            severity: "all",
            notify_on_resolve_case: true,
        },
        CreateCase {
            label: "the longest legal ids and channel name",
            team_id: "T".repeat(256),
            channel_id: "C".repeat(256),
            channel_name: Some("n".repeat(256)),
            severity: "none",
            notify_on_resolve_case: false,
        },
    ]
}

/// Assert `reply` is the modelled `ConflictException`: 409, the error type on the wire,
/// and a `message` built from the request's `team_id` and the account the emulator
/// presents, so a caller reads which workspace is missing.
fn assert_conflict(label: &str, team_id: &str, reply: &crate::slack_channel_configurations::Reply) {
    assert_eq!(
        reply.status, 409,
        "case {label:?}: expected HTTP 409, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ConflictException",
        "case {label:?}: expected x-amzn-errortype ConflictException, got {:?}",
        reply.error_type
    );
    let message = reply.string("message").unwrap_or_default();
    assert!(
        message.contains(team_id) && message.contains(engine::emulator_account_id()),
        "case {label:?}: expected a message naming account {} and team {team_id:?}, got {:?}",
        engine::emulator_account_id(),
        message
    );
}

#[tokio::test]
async fn a_channel_configuration_needs_a_registered_workspace() {
    for case in create_cases() {
        let label = case.label;
        let emulator = Emulator::new();

        // No workspace is registered for the account, so there is nothing to configure
        // a channel in.
        let refused = emulator.create(case.body()).await;
        assert_conflict(label, &case.team_id, &refused);

        // And the refused create stored nothing: the listing is empty.
        let listed = emulator.list(None).await;
        assert_eq!(
            listed.configurations(),
            Vec::<Value>::new(),
            "case {label:?}: a refused create must store nothing, got {}",
            listed.body
        );

        // Registering the workspace the request names is what satisfies the
        // precondition: the very same request is accepted and read back by the listing.
        let registered = emulator.register_workspace(&case.team_id).await;
        assert_eq!(
            registered.status, 200,
            "case {label:?}: expected the registration to answer HTTP 200, got {} / {}",
            registered.status, registered.body
        );
        let created = emulator.create(case.body()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to succeed once the workspace was \
             registered, got {} / {}",
            created.status, created.body
        );
        let stored = emulator.listed(&case.team_id, &case.channel_id).await;
        assert!(
            stored.is_some(),
            "case {label:?}: expected the created configuration to be listed, got {}",
            emulator.list(None).await.body
        );
    }
}

#[tokio::test]
async fn a_workspace_registered_for_another_team_is_not_the_requested_one() {
    // The precondition is per `teamId`: a registry that holds some other workspace does
    // not let a channel be configured in the one the request names. Rows differ in which
    // workspace is registered and which one the create asks for, including the shortest
    // and longest legal ids and a prefix of the requested id.
    let pairs: [(&str, String, String); 4] = [
        (
            "an unrelated workspace is registered",
            "T012ABCDEFG".to_string(),
            "T999ZZZZZZZ".to_string(),
        ),
        (
            "the registered id is a prefix of the requested one",
            "T012ABCDEFG".to_string(),
            "T012ABCDEFGH".to_string(),
        ),
        (
            "the shortest legal id is registered",
            "T".to_string(),
            "C".to_string(),
        ),
        (
            "the longest legal id is registered",
            "T".repeat(256),
            "T".repeat(255),
        ),
    ];

    for (label, registered_team, requested_team) in pairs {
        let emulator = Emulator::new();
        assert_eq!(
            emulator.register_workspace(&registered_team).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );

        let refused = emulator
            .create(json!({
                "teamId": requested_team,
                "channelId": "C01234A5BCD",
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_conflict(label, &requested_team, &refused);

        // The registered workspace still accepts a channel, so the refusal above is
        // about the workspace the request named and not about the registry being
        // unreadable.
        let created = emulator
            .create(json!({
                "teamId": registered_team,
                "channelId": "C01234A5BCD",
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected a channel in the registered workspace to be \
             accepted, got {} / {}",
            created.status, created.body
        );
    }
}

#[tokio::test]
async fn registering_a_workspace_reports_it_and_is_bounded_by_the_documented_quota() {
    let emulator = Emulator::new();
    // Rows differ in team id form and length, and cover both published bounds of
    // `teamId` (`len 1..256 pattern ^\S+$`).
    let teams: Vec<String> = vec![
        "T012ABCDEFG".to_string(),
        "T".to_string(),
        "T-mixed.Case_9".to_string(),
        "T".repeat(256),
        "T0000000000".to_string(),
    ];
    assert_eq!(
        teams.len(),
        MAX_SLACK_WORKSPACE_CONFIGURATIONS,
        "the table must fill exactly the documented workspace quota"
    );

    for team in &teams {
        let label = format!("register {}", &team[..team.len().min(16)]);
        // Registering twice reports the same record: a `teamId` identifies a workspace,
        // so the second call adds nothing.
        for attempt in 1..=2 {
            let reply = emulator.register_workspace(team).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?} attempt {attempt}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.string("teamId").as_deref(),
                Some(team.as_str()),
                "case {label:?} attempt {attempt}: expected the registered teamId, got {}",
                reply.body
            );
            let account_type = reply.string("accountType");
            assert_eq!(
                account_type.as_deref(),
                Some(ACCOUNT_TYPE),
                "case {label:?} attempt {attempt}: expected accountType {ACCOUNT_TYPE:?}, \
                 got {account_type:?}"
            );
            assert!(
                ACCOUNT_TYPE_VALUES.contains(&ACCOUNT_TYPE),
                "the reported accountType must be one the model declares, got \
                 {ACCOUNT_TYPE:?} outside {ACCOUNT_TYPE_VALUES:?}"
            );
            // A registered workspace has a name, which this operation reports, and the
            // registration recorded it, so both attempts report the same one. The rule
            // itself is covered in [`crate::slack_workspace_team_name`].
            assert_eq!(
                reply.string("teamName").as_deref(),
                Some(derived_slack_workspace_name(team).as_str()),
                "case {label:?} attempt {attempt}: expected the recorded teamName, got {}",
                reply.body
            );
        }

        // The registration is what the create precondition reads.
        let created = emulator
            .create(json!({
                "teamId": team,
                "channelId": "C01234A5BCD",
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected a channel in the registered workspace to be \
             accepted, got {} / {}",
            created.status, created.body
        );
    }

    // One workspace past the documented quota is refused, and the refusal does not
    // register it: a channel in it is still the missing-workspace conflict.
    for over_quota in ["T999ZZZZZZZ", "Z"] {
        let refused = emulator.register_workspace(over_quota).await;
        assert_eq!(
            refused.status, 400,
            "case {over_quota:?}: expected HTTP 400 past the workspace quota, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {over_quota:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        let created = emulator
            .create(json!({
                "teamId": over_quota,
                "channelId": "C01234A5BCD",
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await;
        assert_conflict(over_quota, over_quota, &created);
    }
}

#[tokio::test]
async fn deleting_the_account_alias_leaves_the_account_with_none() {
    // Rows span `accountAlias`'s charset and both published length bounds: word
    // characters, a hyphen, a space, digits, an underscore, the shortest legal alias
    // and the longest.
    let aliases: Vec<String> = vec![
        "my-team".to_string(),
        "Support Team 1".to_string(),
        "under_score".to_string(),
        "0123456789".to_string(),
        "a".repeat(ACCOUNT_ALIAS_MIN_LEN),
        "z".repeat(ACCOUNT_ALIAS_MAX_LEN),
    ];

    for alias in aliases {
        let label = alias.clone();
        let emulator = Emulator::new();

        assert_eq!(
            emulator.put_alias(&alias).await.status,
            200,
            "case {label:?}: the alias must be put"
        );
        assert_eq!(
            emulator.get_alias().await.string("accountAlias").as_deref(),
            Some(alias.as_str()),
            "case {label:?}: expected the put alias to be readable before the delete"
        );

        let deleted = emulator.delete_alias().await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete to answer HTTP 200, got {} / {}",
            deleted.status, deleted.body
        );

        // The account now holds no alias, so the optional output member is absent and
        // the body is an empty object.
        let after = emulator.get_alias().await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: expected HTTP 200 after the delete, got {} / {}",
            after.status, after.body
        );
        assert_eq!(
            after.body,
            json!({}),
            "case {label:?}: expected an empty body after the delete, got {}",
            after.body
        );

        // Reading again reports the same thing: a read changes nothing.
        let again = emulator.get_alias().await;
        assert_eq!(
            again.body, after.body,
            "case {label:?}: a second get reported {} after the first reported {}",
            again.body, after.body
        );

        // There is nothing left to delete, which is what this operation's
        // `ResourceNotFoundException` is bound for.
        let twice = emulator.delete_alias().await;
        assert_eq!(
            twice.status, 404,
            "case {label:?}: expected HTTP 404 for a second delete, got {} / {}",
            twice.status, twice.body
        );
        assert_eq!(
            twice.error_type, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            twice.error_type
        );

        // And the account can be aliased again: the delete removed the alias, not the
        // ability to hold one.
        assert_eq!(
            emulator.put_alias(&alias).await.status,
            200,
            "case {label:?}: the alias must be puttable again"
        );
        assert_eq!(
            emulator.get_alias().await.string("accountAlias").as_deref(),
            Some(alias.as_str()),
            "case {label:?}: expected the re-put alias to be readable"
        );
    }
}

#[tokio::test]
async fn deleting_an_alias_that_was_never_put_is_not_found() {
    let emulator = Emulator::new();

    // A fresh account holds no alias, so the read reports none and there is nothing to
    // delete.
    let before = emulator.get_alias().await;
    assert_eq!(
        before.body,
        json!({}),
        "expected an empty body before any alias was put, got {}",
        before.body
    );
    for attempt in 1..=2 {
        let refused = emulator.delete_alias().await;
        assert_eq!(
            refused.status, 404,
            "attempt {attempt}: expected HTTP 404 with no alias stored, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ResourceNotFoundException",
            "attempt {attempt}: expected ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        // The not-found message is built from the account the emulator presents, so a
        // caller reads which account has no alias.
        let message = refused.string("message").unwrap_or_default();
        assert!(
            message.contains(engine::emulator_account_id()),
            "attempt {attempt}: expected the message to name account {}, got {message:?}",
            engine::emulator_account_id()
        );
    }

    // A refused delete did not create an alias either.
    assert!(
        emulator.get_alias().await.string("accountAlias").is_none(),
        "expected no alias after the refused deletes"
    );
}

#[tokio::test]
async fn deleting_the_alias_touches_nothing_else_in_the_account() {
    let emulator = Emulator::new();
    let team_id = "T012ABCDEFG";
    assert_eq!(
        emulator.register_workspace(team_id).await.status,
        200,
        "the workspace must be registered"
    );
    let created = emulator
        .create(json!({
            "teamId": team_id,
            "channelId": "C01234A5BCD",
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the configuration must be created, got {} / {}",
        created.status, created.body
    );
    assert_eq!(
        emulator.put_alias("my-team").await.status,
        200,
        "the alias must be put"
    );

    assert_eq!(
        emulator.delete_alias().await.status,
        200,
        "the alias must be deleted"
    );

    // The channel configuration and the workspace registration are separate resources:
    // deleting the alias leaves both.
    assert!(
        emulator.listed(team_id, "C01234A5BCD").await.is_some(),
        "expected the channel configuration to survive the alias delete, got {}",
        emulator.list(None).await.body
    );
    let another = emulator
        .create(json!({
            "teamId": team_id,
            "channelId": "C99999ZZZZZ",
            "notifyOnCaseSeverity": "none",
            "channelRoleArn": role_arn("SupportAppRole"),
        }))
        .await;
    assert_eq!(
        another.status, 200,
        "expected the workspace registration to survive the alias delete, got {} / {}",
        another.status, another.body
    );
}
