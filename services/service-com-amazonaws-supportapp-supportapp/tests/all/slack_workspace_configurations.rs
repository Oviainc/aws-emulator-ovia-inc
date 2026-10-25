// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListSlackWorkspaceConfigurations`.
//!
//! The rule under test: the listing reports exactly the Slack workspaces this account
//! has registered, each from its stored record, in `teamId` order, in ONE response —
//! the registry is bounded by the documented workspace quota and the operation takes no
//! page size — and it changes nothing, so no `nextToken` is ever returned and a token a
//! request carries names no position and is refused.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]: only the
//! HTTP status, the `x-amzn-errortype` header and the JSON body are inspected, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The tests are tables whose rows differ along what the rule generalises over: the
//! `teamId` form and length (both published `len 1..256` bounds), how many workspaces
//! are registered (none, one, several, the documented maximum), and the form of a
//! pagination token a request sends. The recorded `T012ABCDEFG` case is one row of
//! many.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    derived_slack_workspace_name, MAX_SLACK_WORKSPACE_CONFIGURATIONS,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="RegisterSlackWorkspaceForOrganization")`).
const TEAM_ID_MIN_LEN: usize = 1;
const TEAM_ID_MAX_LEN: usize = 256;

/// Ask `ListSlackWorkspaceConfigurations` for the listing.
///
/// `pub(crate)` so the sibling suites in this test target read the registry through
/// this one helper instead of each spelling the route and the body again.
pub(crate) async fn list_workspaces(emulator: &Emulator, next_token: Option<&str>) -> Reply {
    let body = match next_token {
        Some(token) => json!({ "nextToken": token }),
        None => json!({}),
    };
    emulator
        .call("list-slack-workspace-configurations", body)
        .await
}

/// The `slackWorkspaceConfigurations` member of a listing reply.
pub(crate) fn reported(reply: &Reply) -> Vec<Value> {
    reply
        .body
        .get("slackWorkspaceConfigurations")
        .filter(|value| !value.is_null())
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every record the listing reports, in the order it reported them, asserting it came
/// back WHOLE: one HTTP 200 with no `nextToken`.
async fn listed_workspaces(emulator: &Emulator, label: &str) -> Vec<Value> {
    let reply = list_workspaces(emulator, None).await;
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} / {}",
        reply.status, reply.body
    );
    assert!(
        reply.string("nextToken").is_none(),
        "case {label:?}: expected no nextToken on a registry the workspace quota bounds, got {}",
        reply.body
    );
    reported(&reply)
}

/// The `teamId` of each reported record, in the order the listing reported them.
pub(crate) fn reported_team_ids(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            record
                .get("teamId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The team ids the tables register: rows differ in form and length and cover both
/// published `teamId` bounds. There are exactly as many as the documented workspace
/// quota, so a table may register all of them.
///
/// `pub(crate)` so the sibling suites in this test target register the same spread of
/// workspaces instead of each spelling its own.
pub(crate) fn team_ids() -> Vec<String> {
    let ids = vec![
        // The recorded case: one row of many.
        "T012ABCDEFG".to_string(),
        "T".repeat(TEAM_ID_MIN_LEN),
        "T-mixed.Case_9".to_string(),
        "T999ZZZZZZZ".to_string(),
        "T".repeat(TEAM_ID_MAX_LEN),
    ];
    assert_eq!(
        ids.len(),
        MAX_SLACK_WORKSPACE_CONFIGURATIONS,
        "the table must fit exactly the documented workspace quota"
    );
    ids
}

#[tokio::test]
async fn a_registered_workspace_is_reported_by_list_exactly_as_stored() {
    for team_id in team_ids() {
        let label = format!("team {}", &team_id[..team_id.len().min(16)]);
        let emulator = Emulator::new();

        // Nothing is registered yet, so the account's listing is empty — and it is a
        // list, not an absent member.
        let empty = list_workspaces(&emulator, None).await;
        assert_eq!(
            empty.status, 200,
            "case {label:?}: expected HTTP 200 for an account with no workspace, got {} / {}",
            empty.status, empty.body
        );
        assert_eq!(
            reported(&empty),
            Vec::<Value>::new(),
            "case {label:?}: expected no records before any registration, got {}",
            empty.body
        );
        assert!(
            empty.string("nextToken").is_none(),
            "case {label:?}: expected no nextToken on the whole listing, got {}",
            empty.body
        );

        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );

        let records = listed_workspaces(&emulator, &label).await;
        assert_eq!(
            reported_team_ids(&records),
            vec![team_id.clone()],
            "case {label:?}: expected exactly the registered workspace, got {records:?}"
        );
        // Registering a workspace FOR THE ORGANIZATION is what lets member accounts
        // authorize it, so that is what the stored record carries and the listing
        // reports.
        assert_eq!(
            records[0].get("allowOrganizationMemberAccount"),
            Some(&json!(true)),
            "case {label:?}: expected allowOrganizationMemberAccount true, got {:?}",
            records[0]
        );
        // A registered workspace has a name, recorded when it was registered, so the
        // listing reports it from the stored record. The rule itself is covered in
        // [`crate::slack_workspace_team_name`].
        assert_eq!(
            records[0].get("teamName").and_then(Value::as_str),
            Some(derived_slack_workspace_name(&team_id).as_str()),
            "case {label:?}: expected the recorded teamName, got {:?}",
            records[0]
        );
    }
}

#[tokio::test]
async fn the_listing_reports_every_registered_workspace_in_one_response() {
    // Rows differ in how many workspaces are registered: none, one, several, one short
    // of the documented maximum, and that maximum itself. Whatever the count, the
    // account cannot register more than its quota, so the listing is never split.
    let all = team_ids();
    for count in [
        0,
        1,
        2,
        MAX_SLACK_WORKSPACE_CONFIGURATIONS - 1,
        MAX_SLACK_WORKSPACE_CONFIGURATIONS,
    ] {
        let label = format!("{count} registered");
        let emulator = Emulator::new();
        for team_id in all.iter().take(count) {
            assert_eq!(
                emulator.register_workspace(team_id).await.status,
                200,
                "case {label:?}: workspace {} must be registered",
                &team_id[..team_id.len().min(16)]
            );
        }

        // The store is keyed by `teamId`, so the listing walks the registry in that
        // order whatever order the registrations arrived in.
        let mut expected: Vec<String> = all.iter().take(count).cloned().collect();
        expected.sort();

        let records = listed_workspaces(&emulator, &label).await;
        assert_eq!(
            reported_team_ids(&records),
            expected,
            "case {label:?}: expected every registered workspace exactly once in teamId \
             order, got {:?}",
            reported_team_ids(&records)
        );
    }
}

#[tokio::test]
async fn a_next_token_no_listing_returned_is_refused() {
    let emulator = Emulator::new();
    for team_id in team_ids() {
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "the workspace must be registered"
        );
    }
    // A channel configuration too, so each refusal is about the token rather than about
    // an empty account.
    assert_eq!(
        emulator
            .create(json!({
                "teamId": "T012ABCDEFG",
                "channelId": "C01234A5BCD",
                "notifyOnCaseSeverity": "none",
                "channelRoleArn": role_arn("SupportAppRole"),
            }))
            .await
            .status,
        200,
        "the channel configuration must be created"
    );

    // Rows span the forms a client could send a token in: plain text, hex of an id, hex
    // of a position, hex of bytes that are not text, and an id sent unencoded. No
    // listing of this service returns a token, so every one of them names no position.
    let tokens = [
        ("not hex", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not text", "fffe".to_string()),
        ("hex of nothing", hex::encode("")),
        ("hex of a team id", hex::encode("T012ABCDEFG")),
        ("hex of a position", hex::encode("2")),
        ("hex of a negative position", hex::encode("-1")),
        ("hex of a padded position", hex::encode("007")),
        ("hex of a signed position", hex::encode("+1")),
        ("hex of a fractional position", hex::encode("1.5")),
        ("the team id itself, unencoded", "T012ABCDEFG".to_string()),
    ];
    for (label, token) in tokens {
        let refused = list_workspaces(&emulator, Some(&token)).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected HTTP 400 for token {token:?}, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
    }

    // A listing with no token is still served whole, so the refusals above are about
    // the token and not about the listing being refused outright.
    let records = listed_workspaces(&emulator, "no token").await;
    let mut expected = team_ids();
    expected.sort();
    assert_eq!(
        reported_team_ids(&records),
        expected,
        "expected the untokened listing to report every registered workspace, got {:?}",
        reported_team_ids(&records)
    );
}

#[tokio::test]
async fn listing_the_workspaces_changes_nothing() {
    let emulator = Emulator::new();
    for team_id in team_ids() {
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "the workspace must be registered"
        );
    }

    // Repeated reads report the same thing: a listing is not a mutation.
    let first = list_workspaces(&emulator, None).await;
    let again = list_workspaces(&emulator, None).await;
    assert_eq!(
        again.body, first.body,
        "a second listing reported {} after the first reported {}",
        again.body, first.body
    );
    let read_again = listed_workspaces(&emulator, "after repeated reads").await;
    let mut expected = team_ids();
    expected.sort();
    assert_eq!(
        reported_team_ids(&read_again),
        expected,
        "expected every workspace to survive the reads, got {:?}",
        reported_team_ids(&read_again)
    );

    // And the registry still holds its quota: the reads consumed none of it, so a
    // further registration is still the one refused as past the quota.
    let over_quota = emulator.register_workspace("Z").await;
    assert_eq!(
        over_quota.status, 400,
        "expected the quota to be unchanged by the reads, got {} / {}",
        over_quota.status, over_quota.body
    );
}

#[tokio::test]
async fn the_workspace_registry_and_the_channel_configurations_are_separate_resources() {
    let emulator = Emulator::new();
    let team_id = "T012ABCDEFG";
    assert_eq!(
        emulator.register_workspace(team_id).await.status,
        200,
        "the workspace must be registered"
    );

    // Rows: several channel configurations inside the one registered workspace. However
    // many channels the account configures, it has registered exactly one workspace.
    for channel_id in ["C01234A5BCD", "C99999ZZZZZ", "C"] {
        assert_eq!(
            emulator
                .create(json!({
                    "teamId": team_id,
                    "channelId": channel_id,
                    "notifyOnCaseSeverity": "none",
                    "channelRoleArn": role_arn("SupportAppRole"),
                }))
                .await
                .status,
            200,
            "case {channel_id:?}: the channel configuration must be created"
        );
        let records = listed_workspaces(&emulator, channel_id).await;
        assert_eq!(
            reported_team_ids(&records),
            vec![team_id.to_string()],
            "case {channel_id:?}: expected the one registered workspace, got {records:?}"
        );
    }

    // The two listings report different resources: the channel listing reports the
    // channels, the workspace listing the workspace.
    assert_eq!(
        emulator.list(None).await.configurations().len(),
        3,
        "expected the three channel configurations to be listed, got {}",
        emulator.list(None).await.body
    );
}

