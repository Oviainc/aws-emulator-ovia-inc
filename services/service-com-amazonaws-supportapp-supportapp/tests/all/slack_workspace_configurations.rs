// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListSlackWorkspaceConfigurations`.
//!
//! The rule under test: the listing reports exactly the Slack workspaces this account
//! has registered, each from its stored record, in `teamId` order, a page at a time,
//! and it changes nothing — so a token it did not mint names no position and is
//! refused.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]: only the
//! HTTP status, the `x-amzn-errortype` header and the JSON body are inspected, so a
//! response that is not computed from persisted state cannot pass.
//!
//! The tests are tables whose rows differ along what the rule generalises over: the
//! `teamId` form and length (both published `len 1..256` bounds), how many workspaces
//! are registered (none, one, exactly a page, one past a page, the documented
//! maximum), and the form of a bogus pagination token. The recorded `T012ABCDEFG` case
//! is one row of many.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    MAX_SLACK_WORKSPACE_CONFIGURATIONS, WORKSPACE_PAGE_SIZE,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};

/// `teamId` is `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="RegisterSlackWorkspaceForOrganization")`).
const TEAM_ID_MIN_LEN: usize = 1;
const TEAM_ID_MAX_LEN: usize = 256;

/// `paginationToken`, the target of every `nextToken` of this service, is
/// `len 1..256, pattern ^\S+$`
/// (`smithy_query(query="operation", operation="ListSlackWorkspaceConfigurations")`).
const PAGINATION_TOKEN_MAX_LEN: usize = 256;

/// `ListSlackChannelConfigurations` reports at most this many configurations per page
/// (`CHANNEL_PAGE_SIZE` in `src/state.rs`; the service publishes no page size).
const CHANNEL_PAGE_SIZE: usize = 10;

/// Assert a minted `nextToken` is one the model lets this service put on the wire: a
/// token past the published bound is rejected by the client before the caller can
/// send it back, which would make the page it points at unreachable.
fn assert_token_is_modelled(label: &str, token: &str) {
    assert!(
        !token.is_empty() && token.len() <= PAGINATION_TOKEN_MAX_LEN,
        "case {label:?}: expected a nextToken of 1..={PAGINATION_TOKEN_MAX_LEN} \
         characters, got {} characters",
        token.len()
    );
    assert!(
        !token.chars().any(char::is_whitespace),
        "case {label:?}: expected a nextToken matching ^\\S+$, got {token:?}"
    );
}

/// Ask `ListSlackWorkspaceConfigurations` for one page.
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

/// Walk every page the listing offers and return the reported records in order,
/// asserting each page honours the page size and that the walk terminates.
async fn drain_workspace_pages(emulator: &Emulator, label: &str) -> Vec<Value> {
    let mut records = Vec::new();
    let mut token: Option<String> = None;
    // One more round trip than there can be pages, so a cursor that never terminates
    // fails here instead of looping forever.
    let max_pages = MAX_SLACK_WORKSPACE_CONFIGURATIONS / WORKSPACE_PAGE_SIZE + 2;
    for page_index in 0..=max_pages {
        let page = list_workspaces(emulator, token.as_deref()).await;
        assert_eq!(
            page.status, 200,
            "case {label:?} page {page_index}: expected HTTP 200, got {} / {}",
            page.status, page.body
        );
        let on_page = reported(&page);
        assert!(
            on_page.len() <= WORKSPACE_PAGE_SIZE,
            "case {label:?} page {page_index}: expected at most {WORKSPACE_PAGE_SIZE} \
             records per page, got {}",
            on_page.len()
        );
        records.extend(on_page);
        match page.string("nextToken") {
            Some(next) => {
                assert_token_is_modelled(label, &next);
                token = Some(next);
            }
            None => return records,
        }
    }
    panic!("case {label:?}: the listing did not terminate after {max_pages} pages");
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
fn team_ids() -> Vec<String> {
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
            "case {label:?}: expected no nextToken on the only page, got {}",
            empty.body
        );

        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {label:?}: the workspace must be registered"
        );

        let records = drain_workspace_pages(&emulator, &label).await;
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
        // The workspace's Slack name is not in the request and the emulator has no
        // Slack to ask, so the optional member is absent rather than invented.
        assert!(
            records[0]
                .get("teamName")
                .is_none_or(|value| value.is_null()),
            "case {label:?}: expected no teamName, got {:?}",
            records[0]
        );
    }
}

#[tokio::test]
async fn pagination_reports_every_registered_workspace_exactly_once() {
    // Rows differ in how many workspaces are registered: none, one, exactly a page, one
    // past a page, and the account's documented maximum.
    let all = team_ids();
    for count in [
        0,
        1,
        WORKSPACE_PAGE_SIZE,
        WORKSPACE_PAGE_SIZE + 1,
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

        let records = drain_workspace_pages(&emulator, &label).await;
        assert_eq!(
            reported_team_ids(&records),
            expected,
            "case {label:?}: expected every registered workspace exactly once in teamId \
             order, got {:?}",
            reported_team_ids(&records)
        );

        // A nextToken is offered exactly when a further page exists.
        let first = list_workspaces(&emulator, None).await;
        assert_eq!(
            first.string("nextToken").is_some(),
            count > WORKSPACE_PAGE_SIZE,
            "case {label:?}: expected a nextToken only past a full page, got {}",
            first.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    for team_id in team_ids().into_iter().take(WORKSPACE_PAGE_SIZE + 1) {
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "the workspace must be registered"
        );
    }
    // A channel configuration, so the channel listing has a cursor of its own to mint.
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

    // Rows: not hex at all, odd-length hex, hex that decodes to bytes that are not
    // text, hex of nothing, hex of a team id, and hex of a position spelled in a way
    // this service never mints.
    let bogus = [
        ("not hex", "not-a-token".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not text", "fffe".to_string()),
        ("hex of nothing", hex::encode("")),
        ("hex of a team id", hex::encode("T012ABCDEFG")),
        ("hex of a negative position", hex::encode("-1")),
        ("hex of a padded position", hex::encode("007")),
        ("hex of a signed position", hex::encode("+1")),
        ("hex of a fractional position", hex::encode("1.5")),
        ("the team id itself, unencoded", "T012ABCDEFG".to_string()),
    ];
    for (label, token) in bogus {
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

    // A token this service DID mint walks the pages, so the refusals above are about
    // the tokens and not about pagination being broken.
    let first = list_workspaces(&emulator, None).await;
    let token = first
        .string("nextToken")
        .expect("more workspaces are registered than fit on one page");
    let second = list_workspaces(&emulator, Some(&token)).await;
    assert_eq!(
        second.status, 200,
        "expected the minted token to be accepted, got {} / {}",
        second.status, second.body
    );
    assert!(
        !reported(&second).is_empty(),
        "expected the second page to report the remaining workspace, got {}",
        second.body
    );
}

#[tokio::test]
async fn every_minted_next_token_satisfies_the_modelled_pagination_token_constraints() {
    // Both listings mint their cursor through the one codec in `src/state.rs`, and the
    // ids a page boundary falls on are what a key-derived token would be built from.
    // Rows: the longest legal `teamId`, and channel ids at both published bounds, with
    // enough records stored for each listing to offer a token.
    let emulator = Emulator::new();
    let long_team = "T".repeat(TEAM_ID_MAX_LEN);
    for team_id in team_ids() {
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "the workspace must be registered"
        );
    }
    // One past a channel page, so the channel listing offers a token too, with the
    // longest legal ids at the boundary.
    for index in 0..=CHANNEL_PAGE_SIZE {
        let channel_id = format!("C{}", "c".repeat(index * 20));
        assert_eq!(
            emulator
                .create(json!({
                    "teamId": long_team,
                    "channelId": channel_id,
                    "notifyOnCaseSeverity": "none",
                    "channelRoleArn": role_arn("SupportAppRole"),
                }))
                .await
                .status,
            200,
            "case {index}: the channel configuration must be created"
        );
    }

    let workspaces = list_workspaces(&emulator, None).await;
    let workspace_token = workspaces
        .string("nextToken")
        .expect("more workspaces are registered than fit on one page");
    assert_token_is_modelled("the workspace listing", &workspace_token);
    let resumed = list_workspaces(&emulator, Some(&workspace_token)).await;
    assert_eq!(
        resumed.status, 200,
        "expected the minted workspace token to be accepted, got {} / {}",
        resumed.status, resumed.body
    );

    let channels = emulator.list(None).await;
    let channel_token = channels
        .string("nextToken")
        .expect("more channel configurations are stored than fit on one page");
    assert_token_is_modelled("the channel listing", &channel_token);
    let resumed = emulator.list(Some(&channel_token)).await;
    assert_eq!(
        resumed.status, 200,
        "expected the minted channel token to be accepted, got {} / {}",
        resumed.status, resumed.body
    );
    assert_eq!(
        resumed.configurations().len(),
        1,
        "expected the last channel configuration on the second page, got {}",
        resumed.body
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
    let drained = drain_workspace_pages(&emulator, "after repeated reads").await;
    let mut expected = team_ids();
    expected.sort();
    assert_eq!(
        reported_team_ids(&drained),
        expected,
        "expected every workspace to survive the reads, got {:?}",
        reported_team_ids(&drained)
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
        let records = drain_workspace_pages(&emulator, channel_id).await;
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

