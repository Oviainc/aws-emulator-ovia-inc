// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the name a registered Slack workspace carries.
//!
//! The rule under test: a registered Slack workspace HAS a name, and every operation
//! that reports a workspace configuration reports that name — the registration records
//! one for the workspace, so `RegisterSlackWorkspaceForOrganization` and
//! `ListSlackWorkspaceConfigurations` report the identical value for the same workspace,
//! every time, and the value is one the model lets this service put on the wire.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::slack_channel_configurations`]: only the HTTP
//! status and the JSON body are inspected, so a name that is not read back out of
//! persisted state cannot pass.
//!
//! The tables' rows differ along what the rule generalises over: the `teamId` form,
//! charset and length (both published `len 1..256` bounds, a multi-byte id, and lengths
//! either side of the point where a name has to be clipped to the modelled maximum), how
//! many workspaces are registered at once, and how often each is registered and read. The
//! recorded `T012ABCDEFG` case is one row of many.

use serde_json::Value;
use service_com_amazonaws_supportapp_supportapp::state::MAX_TEAM_NAME_LEN;

use crate::slack_channel_configurations::{Emulator, Reply};
use crate::slack_workspace_configurations::{list_workspaces, reported, team_ids};

/// `teamName` is `len 1..256, pattern ^.+$` on both
/// `RegisterSlackWorkspaceForOrganizationResult` and `SlackWorkspaceConfiguration`
/// (`smithy_query(query="shape", shape="teamName")`).
const TEAM_NAME_MIN_LEN: usize = 1;

/// Team ids whose rows differ in form, charset and length, including the two published
/// `teamId` bounds (`len 1..256, pattern ^\S+$`) and lengths either side of the point
/// where appending anything to an id would leave the modelled `teamName` maximum.
fn named_team_ids() -> Vec<(&'static str, String)> {
    vec![
        // The recorded case: one row of many.
        ("the recorded team id", "T012ABCDEFG".to_string()),
        ("the shortest legal team id", "T".to_string()),
        ("mixed case and punctuation", "T-mixed.Case_9".to_string()),
        ("multi-byte characters", "T-Ünïcødé-漢字".to_string()),
        ("one short of the longest", "T".repeat(MAX_TEAM_NAME_LEN - 1)),
        ("the longest legal team id", "T".repeat(MAX_TEAM_NAME_LEN)),
    ]
}

/// The `teamName` a reply reports at the top level, as a workspace registration does.
fn registered_name(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from the registration, got {} / {}",
        reply.status, reply.body
    );
    let name = reply
        .string("teamName")
        .unwrap_or_else(|| panic!("case {case:?}: expected a teamName, got {}", reply.body));
    assert_modelled(case, &name);
    name
}

/// The `teamName` of the one listed record whose `teamId` is `team_id`.
fn listed_name(case: &str, records: &[Value], team_id: &str) -> String {
    let record = records
        .iter()
        .find(|record| record.get("teamId").and_then(Value::as_str) == Some(team_id))
        .unwrap_or_else(|| panic!("case {case:?}: expected the workspace to be listed, got {records:?}"));
    let name = record
        .get("teamName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a teamName on {record:?}"))
        .to_string();
    assert_modelled(case, &name);
    name
}

/// Assert a reported name is one the model lets this service put on the wire: `teamName`
/// is `len 1..256, pattern ^.+$`, so it is neither empty nor past the maximum and carries
/// no line break (which `.` does not match).
fn assert_modelled(case: &str, name: &str) {
    let length = name.chars().count();
    assert!(
        (TEAM_NAME_MIN_LEN..=MAX_TEAM_NAME_LEN).contains(&length),
        "case {case:?}: expected a teamName of {TEAM_NAME_MIN_LEN}..={MAX_TEAM_NAME_LEN} \
         characters, got {length} characters: {name:?}"
    );
    assert!(
        !name.contains('\n') && !name.contains('\r'),
        "case {case:?}: expected a teamName matching ^.+$, got {name:?}"
    );
}

#[tokio::test]
async fn a_registered_workspace_reports_the_same_name_from_register_and_from_the_listing() {
    for (case, team_id) in named_team_ids() {
        let emulator = Emulator::new();
        let registered = registered_name(case, &emulator.register_workspace(&team_id).await);

        // The listing reports the record the registration wrote, so it reports the same
        // name — the member is alive on the wire for every workspace, not only for the
        // call that created it.
        let listed = listed_name(case, &reported(&list_workspaces(&emulator, None).await), &team_id);
        assert_eq!(
            listed, registered,
            "case {case:?}: the listing reported teamName {listed:?} where the registration \
             reported {registered:?}"
        );

        // The name is persisted, not minted per call: registering the workspace again and
        // reading it again report the value already stored.
        for attempt in 2..=3 {
            let again = registered_name(case, &emulator.register_workspace(&team_id).await);
            assert_eq!(
                again, registered,
                "case {case:?} attempt {attempt}: expected teamName {registered:?}, got {again:?}"
            );
            let relisted =
                listed_name(case, &reported(&list_workspaces(&emulator, None).await), &team_id);
            assert_eq!(
                relisted, registered,
                "case {case:?} attempt {attempt}: expected the listing to report \
                 {registered:?}, got {relisted:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_workspace_name_is_determined_by_the_workspace_and_differs_between_workspaces() {
    // Two independent emulators per row: the same workspace gets the same name in both,
    // so the name follows the workspace rather than the call that registered it or the
    // store it was registered in. A fresh pair per row keeps every registration well
    // inside the account's documented workspace quota.
    let mut names = Vec::new();
    for (case, team_id) in named_team_ids() {
        let first = Emulator::new();
        let second = Emulator::new();
        let here = registered_name(case, &first.register_workspace(&team_id).await);
        let there = registered_name(case, &second.register_workspace(&team_id).await);
        assert_eq!(
            here, there,
            "case {case:?}: one emulator reported teamName {here:?} where another reported \
             {there:?} for the same workspace"
        );

        // Deleting the workspace and registering it again is the same workspace again.
        assert_eq!(
            first.delete_workspace(&team_id).await.status,
            200,
            "case {case:?}: the workspace must be deleted"
        );
        let reregistered = registered_name(case, &first.register_workspace(&team_id).await);
        assert_eq!(
            reregistered, here,
            "case {case:?}: expected teamName {here:?} after re-registering, got \
             {reregistered:?}"
        );
        names.push((case, team_id, here));
    }

    // Different workspaces are different workspaces: each reports its own name.
    for (case, _, name) in &names {
        let clashes: Vec<&str> = names
            .iter()
            .filter(|(other_case, _, other)| other_case != case && other == name)
            .map(|(other_case, _, _)| *other_case)
            .collect();
        assert!(
            clashes.is_empty(),
            "case {case:?}: expected a name of its own, but {name:?} is also reported for \
             {clashes:?}"
        );
    }
}

#[tokio::test]
async fn every_workspace_the_listing_reports_carries_its_own_stored_name() {
    // Rows: how many workspaces are registered at once, up to the account's documented
    // quota — so every record of the listing, not only its first, reports a name.
    let all = team_ids();
    for count in 1..=all.len() {
        let case = format!("{count} registered");
        let emulator = Emulator::new();
        let mut expected = Vec::new();
        for team_id in all.iter().take(count) {
            let name = registered_name(&case, &emulator.register_workspace(team_id).await);
            expected.push((team_id.clone(), name));
        }

        let listing = list_workspaces(&emulator, None).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: expected HTTP 200 from the listing, got {} / {}",
            listing.status, listing.body
        );
        let records = reported(&listing);

        assert_eq!(
            records.len(),
            count,
            "case {case:?}: expected {count} listed workspaces, got {}",
            records.len()
        );
        for (team_id, name) in expected {
            let listed = listed_name(&case, &records, &team_id);
            assert_eq!(
                listed, name,
                "case {case:?}: expected teamName {name:?} for the workspace the \
                 registration named, got {listed:?}"
            );
        }
    }
}
