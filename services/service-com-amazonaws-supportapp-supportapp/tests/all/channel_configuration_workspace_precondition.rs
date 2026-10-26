// SPDX-License-Identifier: CC0-1.0

//! The workspace precondition EVERY Slack channel configuration write enforces.
//!
//! A Slack channel configuration lives inside a Slack workspace registered to the
//! account, so a request that adds, updates or deletes one in a workspace this account
//! has not registered is refused with the modelled `ConflictException` (409) — the
//! condition `ConflictException`'s own documentation names first: "Add, update, or
//! delete a Slack channel configuration before you add a Slack workspace to your Amazon
//! Web Services account"
//! (`smithy_query(query="doc", shape="ConflictException")`). The modelled
//! `ResourceNotFoundException` (404) is the other answer: the workspace IS registered
//! and the channel the request names is simply not configured in it.
//!
//! The tests are tables whose rows differ along what the rule generalises over: which
//! of the three writing operations is called (`CreateSlackChannelConfiguration`,
//! `UpdateSlackChannelConfiguration`, `DeleteSlackChannelConfiguration` — every
//! operation whose model binds `ConflictException` and that writes a channel
//! configuration), the form and length of the `teamId` (both published `len 1..256`
//! bounds), and what the account's registry holds: nothing, another workspace, or the
//! requested workspace. The `T99999ZZZZZ` / `C01234A5BCD` delete the issue recorded is
//! one row.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`], so a status
//! or a message that is not computed from the persisted registry cannot pass.

use serde_json::{json, Value};

use crate::error_message_templates::{not_found, refusal};
use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="DeleteSlackChannelConfiguration")`), so
/// the rows below cover both published bounds.
const SLACK_ID_MAX_LEN: usize = 256;

/// The message the unregistered-workspace conflict carries: the condition stated over
/// the account the request was made in and the `teamId` it named.
fn workspace_conflict(team_id: &str) -> String {
    format!(
        "Workspace installation not found for account {account} and team {team_id}",
        account = engine::emulator_account_id()
    )
}

/// Assert `reply` is the modelled `ConflictException` for an unregistered workspace:
/// HTTP 409, the error type on the wire, and the message built from the request.
fn assert_workspace_conflict(case: &str, team_id: &str, reply: &Reply) {
    let message = refusal(case, reply, 409, "ConflictException");
    let expected = workspace_conflict(team_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Assert `reply` is the modelled `ResourceNotFoundException` for a channel that is not
/// configured in a workspace the account HOLDS.
fn assert_channel_not_found(case: &str, team_id: &str, channel_id: &str, reply: &Reply) {
    let message = refusal(case, reply, 404, "ResourceNotFoundException");
    let expected = not_found(team_id, channel_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// A create request for `(team_id, channel_id)` that satisfies every modelled
/// constraint and the documented severity rule, so the only thing left that can refuse
/// it is an account-state rule.
fn create_body(team_id: &str, channel_id: &str) -> Value {
    json!({
        "teamId": team_id,
        "channelId": channel_id,
        "notifyOnCaseSeverity": "none",
        "channelRoleArn": role_arn("SupportAppRole"),
    })
}

/// An update request for `(team_id, channel_id)` that satisfies every modelled
/// constraint and the documented severity rule.
fn update_body(team_id: &str, channel_id: &str) -> Value {
    json!({
        "teamId": team_id,
        "channelId": channel_id,
        "notifyOnCaseSeverity": "none",
    })
}

/// What each of the three channel-configuration writes answers for
/// `(team_id, channel_id)`, named by the operation that answered, so one table row can
/// drive all three.
async fn writes(
    emulator: &Emulator,
    team_id: &str,
    channel_id: &str,
) -> Vec<(&'static str, Reply)> {
    vec![
        (
            "CreateSlackChannelConfiguration",
            emulator.create(create_body(team_id, channel_id)).await,
        ),
        (
            "UpdateSlackChannelConfiguration",
            emulator.update(update_body(team_id, channel_id)).await,
        ),
        (
            "DeleteSlackChannelConfiguration",
            emulator.delete(team_id, channel_id).await,
        ),
    ]
}

/// The team ids the tables run over: the recorded one, both published `@length` bounds,
/// a punctuated id and an all-digit one.
fn team_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the recorded team id", "T99999ZZZZZ".to_string()),
        ("the shortest legal team id", "T".to_string()),
        ("a punctuated mixed-case team id", "T-mixed.Case_9".to_string()),
        ("the longest legal team id", "T".repeat(SLACK_ID_MAX_LEN)),
        ("an all-digit team id", "0123456789".to_string()),
    ]
}

#[tokio::test]
async fn every_channel_configuration_write_needs_a_registered_workspace() {
    for (label, team_id) in team_ids() {
        let emulator = Emulator::new();
        let channel_id = "C01234A5BCD";

        // Nothing is registered, so none of the three writes has a workspace to act in.
        for (operation, reply) in writes(&emulator, &team_id, channel_id).await {
            let case = format!("{label} / {operation} with an empty registry");
            assert_workspace_conflict(&case, &team_id, &reply);
        }
        // And none of them wrote anything.
        assert_eq!(
            emulator.list(None).await.configurations(),
            Vec::<Value>::new(),
            "case {label:?}: a refused write must store nothing"
        );

        // Registering the workspace the requests name is what satisfies the
        // precondition: the create is then accepted, and the two writes that need a
        // configuration report the MISS instead of the conflict.
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );
        let absent = "C99999ZZZZZ";
        assert_channel_not_found(
            &format!("{label} / UpdateSlackChannelConfiguration, workspace registered"),
            &team_id,
            absent,
            &emulator.update(update_body(&team_id, absent)).await,
        );
        assert_channel_not_found(
            &format!("{label} / DeleteSlackChannelConfiguration, workspace registered"),
            &team_id,
            absent,
            &emulator.delete(&team_id, absent).await,
        );

        let created = emulator.create(create_body(&team_id, channel_id)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the create to succeed once the workspace was \
             registered, got {} / {}",
            created.status, created.body
        );
        let updated = emulator.update(update_body(&team_id, channel_id)).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected the update of the created configuration to \
             succeed, got {} / {}",
            updated.status, updated.body
        );
        let deleted = emulator.delete(&team_id, channel_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete of the created configuration to \
             succeed, got {} / {}",
            deleted.status, deleted.body
        );

        // The workspace is still registered, so deleting the same pair again is the
        // miss, not the conflict.
        assert_channel_not_found(
            &format!("{label} / second DeleteSlackChannelConfiguration"),
            &team_id,
            channel_id,
            &emulator.delete(&team_id, channel_id).await,
        );
    }
}

#[tokio::test]
async fn a_workspace_registered_for_another_team_does_not_satisfy_the_precondition() {
    // The precondition is per `teamId`: a registry holding some OTHER workspace does not
    // let a channel configuration be written in the one the request names. Rows differ
    // in which workspace is registered and which one the write asks for, including a
    // prefix of the requested id and both published length bounds.
    let rows: [(&str, String, String); 4] = [
        (
            "an unrelated workspace is registered",
            "T012ABCDEFG".to_string(),
            "T99999ZZZZZ".to_string(),
        ),
        (
            "the registered id is a prefix of the requested one",
            "T012ABCDEFG".to_string(),
            "T012ABCDEFGH".to_string(),
        ),
        (
            "the shortest legal id is registered",
            "T".to_string(),
            "t".to_string(),
        ),
        (
            "the longest legal id is registered",
            "T".repeat(SLACK_ID_MAX_LEN),
            "T".repeat(SLACK_ID_MAX_LEN - 1),
        ),
    ];

    for (label, registered, requested) in rows {
        let emulator = Emulator::new();
        let channel_id = "C01234A5BCD";
        assert_eq!(
            emulator.register_workspace(&registered).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );
        // A configuration in the registered workspace, so the refusals below are about
        // the workspace the request named and not about an empty store.
        assert_eq!(
            emulator
                .create(create_body(&registered, channel_id))
                .await
                .status,
            200,
            "case {label:?}: the configuration in the registered workspace must be created"
        );

        for (operation, reply) in writes(&emulator, &requested, channel_id).await {
            let case = format!("{label} / {operation}");
            assert_workspace_conflict(&case, &requested, &reply);
        }

        // The configuration in the registered workspace is untouched, and its own
        // writes still work.
        assert!(
            emulator.listed(&registered, channel_id).await.is_some(),
            "case {label:?}: the configuration in the registered workspace must survive, got {}",
            emulator.list(None).await.body
        );
        assert_eq!(
            emulator
                .update(update_body(&registered, channel_id))
                .await
                .status,
            200,
            "case {label:?}: expected the update in the registered workspace to succeed"
        );
        assert_eq!(
            emulator.delete(&registered, channel_id).await.status,
            200,
            "case {label:?}: expected the delete in the registered workspace to succeed"
        );
    }
}

#[tokio::test]
async fn removing_the_workspace_makes_its_channel_writes_the_conflict_again() {
    // The precondition is read from the registry at every call, so a workspace the
    // account REMOVES takes the precondition with it: writes in it are the conflict
    // again, whatever was configured in it before. Rows differ in the team id form and
    // in how many channels the workspace held.
    let rows: [(&str, String, &[&str]); 3] = [
        ("one channel", "T012ABCDEFG".to_string(), &["C01234A5BCD"]),
        (
            "three channels",
            "T-mixed.Case_9".to_string(),
            &["C001", "C002", "C003"],
        ),
        (
            "the longest legal team id, one channel",
            "T".repeat(SLACK_ID_MAX_LEN),
            &["C"],
        ),
    ];

    for (label, team_id, channels) in rows {
        let emulator = Emulator::new();
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );
        for channel_id in channels {
            assert_eq!(
                emulator
                    .create(create_body(&team_id, channel_id))
                    .await
                    .status,
                200,
                "case {label:?}: the configuration for {channel_id:?} must be created"
            );
        }

        assert_eq!(
            emulator.delete_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be removable"
        );

        for channel_id in channels {
            for (operation, reply) in writes(&emulator, &team_id, channel_id).await {
                let case = format!("{label} / {operation} on {channel_id} after the workspace went");
                assert_workspace_conflict(&case, &team_id, &reply);
            }
        }

        // Registering the workspace again brings back the precondition, not the
        // configurations that lived in it: each channel is then the miss.
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registrable again"
        );
        for channel_id in channels {
            assert_channel_not_found(
                &format!("{label} / DeleteSlackChannelConfiguration on {channel_id} after re-registering"),
                &team_id,
                channel_id,
                &emulator.delete(&team_id, channel_id).await,
            );
        }
    }
}
