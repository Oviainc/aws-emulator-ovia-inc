// SPDX-License-Identifier: CC0-1.0

//! What a write that names ONE Slack channel configuration answers when the account
//! holds a configuration that only PARTLY matches the pair the request names.
//!
//! `UpdateSlackChannelConfiguration` and `DeleteSlackChannelConfiguration` act on the
//! configuration their `(teamId, channelId)` pair identifies, so each settles that
//! configuration's existence against the account's state before any account-level
//! precondition, and the resource is identified by the WHOLE pair: a stored
//! configuration that shares only one member of it — the same workspace with another
//! channel, or the same channel id in another workspace — is a different resource, so
//! the request is the modelled `ResourceNotFoundException` (HTTP 404), "The specified
//! resource is missing or doesn't exist, such as an account alias, Slack channel
//! configuration, or Slack workspace configuration"
//! (`smithy_query(query="operation", operation="UpdateSlackChannelConfiguration,DeleteSlackChannelConfiguration",
//! model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")`),
//! never the modelled `ConflictException` (409) of an account-level precondition.
//!
//! The sibling suites run that rule over an account holding nothing for the named
//! workspace ([`crate::channel_configuration_lookup_order`]) and over the create path
//! that keeps the conflict ([`crate::channel_configuration_workspace_precondition`]).
//! The table here runs it over the axes those two leave open, so no row's answer can
//! come from an account that simply holds nothing nearby:
//!
//! * how the requested pair RELATES to a configuration the account does hold — sharing
//!   the `teamId`, sharing the `channelId`, a prefix of the stored `channelId`, the
//!   stored `channelId` in another case, or nothing at all;
//! * whether the workspace the request names is itself registered, since the answer is
//!   about the configuration and not about the registry;
//! * whether the account also holds an account alias, the other account-level resource
//!   this service stores.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`], so an answer
//! that is not computed from persisted state cannot pass.

use serde_json::Value;

use crate::channel_configuration_workspace_precondition::{create_body, update_body};
use crate::error_message_templates::{not_found, refusal};
use crate::slack_channel_configurations::{Emulator, Reply};

/// `teamId` and `channelId` are `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="DeleteSlackChannelConfiguration")`), so
/// the rows below reach the published upper bound.
const SLACK_ID_MAX_LEN: usize = 256;

/// One row: the configuration the account holds, and the pair a write names instead.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    /// The pair the account really holds a configuration for.
    stored_team_id: String,
    stored_channel_id: String,
    /// The pair the write names, which the account holds no configuration for.
    team_id: String,
    channel_id: String,
    /// Register the requested workspace too, when it differs from the stored one: the
    /// refusal is about the configuration, so it must not move with the registry.
    register_requested_workspace: bool,
    /// The account alias the account also holds, when it holds one. `accountAlias` is
    /// `len 1..30, pattern ^[\w\- ]+$` (`smithy_query(query="shape", shape="awsAccountAlias")`).
    account_alias: Option<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the same workspace, a channel it holds nothing for",
            stored_team_id: "T012ABCDEFG".to_string(),
            stored_channel_id: "C01234A5BCD".to_string(),
            team_id: "T012ABCDEFG".to_string(),
            channel_id: "C99999ZZZZZ".to_string(),
            register_requested_workspace: false,
            account_alias: None,
        },
        Case {
            label: "the stored channel id, in another registered workspace",
            stored_team_id: "T012ABCDEFG".to_string(),
            stored_channel_id: "C01234A5BCD".to_string(),
            team_id: "T99999ZZZZZ".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            register_requested_workspace: true,
            account_alias: Some("support-team"),
        },
        Case {
            label: "the stored channel id, in another workspace that is not registered",
            stored_team_id: "T012ABCDEFG".to_string(),
            stored_channel_id: "C01234A5BCD".to_string(),
            team_id: "T99999ZZZZZ".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            register_requested_workspace: false,
            account_alias: None,
        },
        Case {
            label: "a prefix of the stored channel id",
            stored_team_id: "T0123456789".to_string(),
            stored_channel_id: "C01234A5BCD".to_string(),
            team_id: "T0123456789".to_string(),
            channel_id: "C01234".to_string(),
            register_requested_workspace: false,
            account_alias: Some("aws support app"),
        },
        Case {
            label: "the stored channel id in another case",
            stored_team_id: "T0123456789".to_string(),
            stored_channel_id: "c01234a5bcd".to_string(),
            team_id: "T0123456789".to_string(),
            channel_id: "C01234A5BCD".to_string(),
            register_requested_workspace: false,
            account_alias: None,
        },
        Case {
            label: "the longest legal channel id beside the shortest stored one",
            stored_team_id: "T".to_string(),
            stored_channel_id: "C".to_string(),
            team_id: "T".to_string(),
            channel_id: "C".repeat(SLACK_ID_MAX_LEN),
            register_requested_workspace: false,
            account_alias: None,
        },
        Case {
            label: "the stored team id in another case, holding the same channel id",
            stored_team_id: "T-mixed.Case_9".to_string(),
            stored_channel_id: "C#9/x".to_string(),
            team_id: "t-mixed.case_9".to_string(),
            channel_id: "C#9/x".to_string(),
            register_requested_workspace: true,
            account_alias: None,
        },
        Case {
            label: "nothing in common with the stored pair, the workspace registered",
            stored_team_id: "T".repeat(SLACK_ID_MAX_LEN),
            stored_channel_id: "C".repeat(SLACK_ID_MAX_LEN),
            team_id: "T0000000000".to_string(),
            channel_id: "C0000000000".to_string(),
            register_requested_workspace: true,
            account_alias: Some("a"),
        },
        Case {
            label: "nothing in common with the stored pair, the workspace not registered",
            stored_team_id: "T0123456789".to_string(),
            stored_channel_id: "C01234A5BCD".to_string(),
            team_id: "T99999ZZZZZ".to_string(),
            channel_id: "C99999ZZZZZ".to_string(),
            register_requested_workspace: false,
            account_alias: Some("Support_App-1"),
        },
    ]
}

/// Bring the account to the state `case` describes: holding exactly one configuration,
/// for the stored pair, plus whatever else the row asks for.
async fn arrange(case: &Case) -> Emulator {
    let label = case.label;
    let emulator = Emulator::new();

    assert_eq!(
        emulator.register_workspace(&case.stored_team_id).await.status,
        200,
        "case {label:?}: the stored configuration's workspace must be registered"
    );
    let created = emulator
        .create(create_body(&case.stored_team_id, &case.stored_channel_id))
        .await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: the stored configuration must be created, got {} / {}",
        created.status, created.body
    );

    if case.register_requested_workspace && case.team_id != case.stored_team_id {
        assert_eq!(
            emulator.register_workspace(&case.team_id).await.status,
            200,
            "case {label:?}: the requested workspace must be registrable"
        );
    }

    if let Some(alias) = case.account_alias {
        let reply = emulator.put_alias(alias).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: the account alias {alias:?} must be storable, got {} / {}",
            reply.status, reply.body
        );
    }

    emulator
}

/// Assert `reply` is the modelled miss for `(team_id, channel_id)`: HTTP 404, the error
/// type on the wire, and the one template this service words a channel miss with.
fn assert_miss(case: &str, team_id: &str, channel_id: &str, reply: &Reply) {
    let message = refusal(case, reply, 404, "ResourceNotFoundException");
    let expected = not_found(team_id, channel_id);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

/// Assert the account still holds exactly the stored configuration: a refused write
/// writes nothing, and it took nothing with it.
async fn assert_only_the_stored_configuration_remains(emulator: &Emulator, case: &Case) {
    let label = case.label;
    assert!(
        emulator
            .listed(&case.stored_team_id, &case.stored_channel_id)
            .await
            .is_some(),
        "case {label:?}: the stored configuration must survive a refused write, got {}",
        emulator.list(None).await.body
    );
    assert!(
        emulator
            .listed(&case.team_id, &case.channel_id)
            .await
            .is_none(),
        "case {label:?}: a refused write must not create the configuration it refused, got {}",
        emulator.list(None).await.body
    );
    let listed = emulator.list(None).await;
    assert_eq!(
        listed.configurations().len(),
        1,
        "case {label:?}: expected exactly 1 stored configuration, got {}",
        listed.body
    );
}

#[tokio::test]
async fn a_write_naming_a_partly_matching_pair_is_the_miss() {
    for case in cases() {
        let emulator = arrange(&case).await;

        // Each write is driven twice: the refusal changed nothing, so the second call
        // answers exactly the same.
        for attempt in 1..=2 {
            assert_miss(
                &format!("{} / update, attempt {attempt}", case.label),
                &case.team_id,
                &case.channel_id,
                &emulator
                    .update(update_body(&case.team_id, &case.channel_id))
                    .await,
            );
            assert_miss(
                &format!("{} / delete, attempt {attempt}", case.label),
                &case.team_id,
                &case.channel_id,
                &emulator.delete(&case.team_id, &case.channel_id).await,
            );
        }

        assert_only_the_stored_configuration_remains(&emulator, &case).await;
    }
}

#[tokio::test]
async fn the_pair_the_account_does_hold_still_answers() {
    // The same two operations, on the same account, for the pair that IS stored: they
    // answer HTTP 200. So the 404 above is read from the account's state rather than
    // assumed from the request, and the pair the row named is the only thing that
    // differs between the two answers.
    for case in cases() {
        let label = case.label;
        let emulator = arrange(&case).await;

        let updated = emulator
            .update(update_body(&case.stored_team_id, &case.stored_channel_id))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected the update of the stored pair to answer HTTP 200, \
             got {} / {}",
            updated.status, updated.body
        );
        let deleted = emulator
            .delete(&case.stored_team_id, &case.stored_channel_id)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete of the stored pair to answer HTTP 200, \
             got {} / {}",
            deleted.status, deleted.body
        );

        // And once it is gone the account holds nothing, so the stored pair is the miss
        // too — with the same template, over the same registry.
        assert_miss(
            &format!("{label} / delete of the stored pair, repeated"),
            &case.stored_team_id,
            &case.stored_channel_id,
            &emulator
                .delete(&case.stored_team_id, &case.stored_channel_id)
                .await,
        );
        let listed = emulator.list(None).await;
        assert_eq!(
            listed.configurations(),
            Vec::<Value>::new(),
            "case {label:?}: expected the account to hold no configuration after the \
             delete, got {}",
            listed.body
        );
    }
}
