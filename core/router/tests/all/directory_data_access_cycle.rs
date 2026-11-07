// SPDX-License-Identifier: CC0-1.0

//! The DS Data FEATURE SWITCH across the two services that share a directory: shutting the
//! data plane and opening it again.
//!
//! `ds:EnableDirectoryDataAccess` and `ds:DisableDirectoryDataAccess` are the only switch
//! that makes the Directory Service DATA API usable for a directory, and
//! `ds:DescribeDirectoryDataAccess` is what reports where the switch stands. The data plane
//! is another crate with another store, so the two can only agree because both reach the one
//! record the control plane writes.
//!
//! The rule this suite pins, in BOTH directions: `ds-data` answers for a directory exactly
//! while the control plane reports its `DataAccessStatus` as `Enabled`, and every call
//! against a directory whose feature is out of service is refused with
//! `AccessDeniedException`, HTTP 403, `Reason` `DATA_DISABLED` and one fixed sentence —
//! identically for an object read, a write and an inventory listing, because what is refused
//! is the directory and not the call. Nothing else about the directory moves: `ds` keeps
//! reporting `Stage` `Active` while its data plane is shut, the objects inside it are the
//! same AD objects after the next enable, and the write the gate refused created nothing.
//!
//! The per-operation surface of that refusal — all 17 routes, crossed with every way the
//! switch can stand — is the data plane's own suite
//! (`services/service-com-amazonaws-directoryservicedata-directoryservicedata/tests/all/data_access_gate.rs`);
//! only this configuration, both services on ONE engine as `core/router` ships them, makes
//! the switch itself observable.

use serde_json::{json, Value};

use super::directory_data_plane_authorization::{create_microsoft_ad, Emulator, PAST_ONE_SPAN};

/// The error the data plane refuses a withdrawn directory with, and the modelled
/// `AccessDeniedReason` it carries
/// (`smithy_query(query="shape", shape="AccessDeniedException")`: `@httpError` 403, members
/// `Message` / `Reason`; `AccessDeniedReason` is `IAM_AUTH`, `DIRECTORY_AUTH`,
/// `DATA_DISABLED`).
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const DATA_DISABLED: &str = "DATA_DISABLED";

/// The whole `Message` that refusal carries: fixed text, naming the control-plane call that
/// turns the feature on and nothing of the request.
const DATA_DISABLED_MESSAGE: &str = "DS Data feature is not enabled on this directory. \
     This feature must first be enabled by calling the EnableDirectoryDataAccess API.";

/// The `DataAccessStatus` values this suite polls for, from the modelled enum
/// (`smithy_query(query="shape", shape="DataAccessStatus")` — `Disabled`, `Disabling`,
/// `Enabled`, `Enabling`, `Failed`).
const DISABLED: &str = "Disabled";
const DISABLING: &str = "Disabling";
const ENABLED: &str = "Enabled";
const ENABLING: &str = "Enabling";

/// The `DirectoryStage` an in-service directory reports throughout
/// (`smithy_query(query="shape", shape="DirectoryStage")` carries it).
const ACTIVE: &str = "Active";

/// How many polls a status this emulator owes the clock gets before the suite gives up: the
/// transitions are sub-second, so this is a bound on a wedged status rather than a wait.
const POLL_ATTEMPTS: usize = 40;

/// The user, group and membership this suite creates while the feature is in service, and
/// the name it tries to create while the directory is shut.
const USER: &str = "s2rSurvivor";
const GROUP: &str = "s2rGroupSurvivor";
const REFUSED_USER: &str = "s2rNeverCreated";

/// The `Stage` the control plane reports for `directory`.
async fn stage(emulator: &Emulator, directory: &str) -> String {
    let described = emulator
        .control_plane(
            "DescribeDirectories",
            json!({"DirectoryIds": [directory]}),
        )
        .await;
    described.body["DirectoryDescriptions"][0]["Stage"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `DataAccessStatus` the control plane reports for `directory`.
async fn data_access_status(emulator: &Emulator, directory: &str) -> String {
    let described = emulator
        .control_plane(
            "DescribeDirectoryDataAccess",
            json!({"DirectoryId": directory}),
        )
        .await;
    described.text("DataAccessStatus")
}

/// Poll until the control plane reports `directory` in `stage_wanted`, as a client waiting
/// for a create does.
async fn await_stage(emulator: &Emulator, directory: &str, stage_wanted: &str) {
    for _ in 0..POLL_ATTEMPTS {
        if stage(emulator, directory).await == stage_wanted {
            return;
        }
        tokio::time::sleep(PAST_ONE_SPAN / 4).await;
    }
    panic!(
        "case waiting for {stage_wanted}: the directory stayed in {:?}",
        stage(emulator, directory).await
    );
}

/// Poll until the control plane reports `status_wanted` for `directory`'s data access, as a
/// client waiting for the enable or the disable to settle does.
async fn await_data_access(emulator: &Emulator, directory: &str, status_wanted: &str) {
    for _ in 0..POLL_ATTEMPTS {
        if data_access_status(emulator, directory).await == status_wanted {
            return;
        }
        tokio::time::sleep(PAST_ONE_SPAN / 4).await;
    }
    panic!(
        "case waiting for DataAccessStatus {status_wanted}: it stayed at {:?}",
        data_access_status(emulator, directory).await
    );
}

/// Drive the control plane's toggle and check it was accepted.
async fn toggle(emulator: &Emulator, operation: &str, directory: &str) {
    let answer = emulator
        .control_plane(operation, json!({"DirectoryId": directory}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {operation}: expected it to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Every string member of a data-plane answer that identifies the object it reports, so two
/// answers can be compared as "the same AD object".
fn identity(answer: &Value) -> Value {
    json!({
        "SID": answer.get("SID"),
        "SAMAccountName": answer.get("SAMAccountName"),
        "DistinguishedName": answer.get("DistinguishedName"),
        "EmailAddress": answer.get("EmailAddress"),
        "Enabled": answer.get("Enabled"),
        "Realm": answer.get("Realm"),
    })
}

/// The `SAMAccountName`s of a paginated data-plane listing, following every `NextToken`.
///
/// `required` carries the members the route requires beyond the paging ones (a membership
/// projection names the group or the member it projects), and the page size is the smallest
/// the model admits so more than one page is walked.
async fn every_name(
    emulator: &Emulator,
    route: &str,
    directory: &str,
    items: &str,
    required: &Value,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = required.clone();
        body["MaxResults"] = json!(1);
        if let Some(token) = &next {
            body["NextToken"] = json!(token);
        }
        let page = emulator.data_plane(route, directory, body).await;
        assert_eq!(
            page.status, 200,
            "case paging {route}: expected every page to be served, got {} with {:?}",
            page.status, page.body
        );
        if let Some(listed) = page.body[items].as_array() {
            names.extend(
                listed
                    .iter()
                    .filter_map(|item| item["SAMAccountName"].as_str())
                    .map(str::to_string),
            );
        }
        next = page.body["NextToken"].as_str().map(str::to_string);
        if next.is_none() {
            return names;
        }
    }
}

/// Assert a data-plane answer is the refusal a withdrawn directory gets: 403
/// `AccessDeniedException`, `Reason` `DATA_DISABLED`, the fixed sentence, and nothing of the
/// request in it.
fn assert_data_disabled(case: &str, answer: &super::directory_data_plane_authorization::Answer) {
    assert_eq!(
        answer.status, 403,
        "case {case}: expected HTTP 403, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        ACCESS_DENIED_EXCEPTION,
        "case {case}: expected {ACCESS_DENIED_EXCEPTION}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body.get("Reason").and_then(Value::as_str),
        Some(DATA_DISABLED),
        "case {case}: expected Reason {DATA_DISABLED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body.get("Message").and_then(Value::as_str),
        Some(DATA_DISABLED_MESSAGE),
        "case {case}: expected Message {DATA_DISABLED_MESSAGE:?}, got {:?}",
        answer.body
    );
}

/// The whole cycle: enable, fill the directory, disable, be refused identically on a read, a
/// write and a listing, then enable again and find the same objects and nothing the refused
/// write left.
#[tokio::test]
async fn disabling_data_access_closes_the_data_plane_without_losing_the_directorys_contents() {
    let emulator = Emulator::new();

    let created = emulator
        .control_plane(
            "CreateMicrosoftAD",
            create_microsoft_ad("cycle.example.com"),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateMicrosoftAD must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let directory = created.text("DirectoryId");
    await_stage(&emulator, &directory, ACTIVE).await;

    // A directory nothing has enabled serves nothing: the switch, not the create, is what
    // opens this API.
    assert_eq!(
        data_access_status(&emulator, &directory).await,
        DISABLED,
        "case a freshly created directory: expected DataAccessStatus {DISABLED}"
    );
    let before_any_enable = emulator
        .data_plane("/Users/ListUsers", &directory, json!({}))
        .await;
    assert_data_disabled("ListUsers before any enable", &before_any_enable);

    toggle(&emulator, "EnableDirectoryDataAccess", &directory).await;
    await_data_access(&emulator, &directory, ENABLED).await;

    // The objects that exist before the disable.
    let created_user = emulator
        .data_plane(
            "/Users/CreateUser",
            &directory,
            json!({
                "SAMAccountName": USER,
                "GivenName": "Sim",
                "Surname": "Survivor",
                "EmailAddress": "s2rSurvivor@cycle.example.com",
            }),
        )
        .await;
    assert_eq!(
        created_user.status, 200,
        "CreateUser on an enabled directory must be served, got {} with {:?}",
        created_user.status, created_user.body
    );
    let created_group = emulator
        .data_plane(
            "/Groups/CreateGroup",
            &directory,
            json!({"SAMAccountName": GROUP}),
        )
        .await;
    assert_eq!(
        created_group.status, 200,
        "CreateGroup on an enabled directory must be served, got {} with {:?}",
        created_group.status, created_group.body
    );
    let joined = emulator
        .data_plane(
            "/GroupMemberships/AddGroupMember",
            &directory,
            json!({"GroupName": GROUP, "MemberName": USER}),
        )
        .await;
    assert_eq!(
        joined.status, 200,
        "AddGroupMember on an enabled directory must be served, got {} with {:?}",
        joined.status, joined.body
    );

    let user_before = emulator
        .data_plane(
            "/Users/DescribeUser",
            &directory,
            json!({"SAMAccountName": USER}),
        )
        .await;
    let group_before = emulator
        .data_plane(
            "/Groups/DescribeGroup",
            &directory,
            json!({"SAMAccountName": GROUP}),
        )
        .await;

    // The disable answers before the directory has left the Data API: the control plane
    // reports the transient status and the data plane is still serving, so the two planes
    // agree at every instant.
    toggle(&emulator, "DisableDirectoryDataAccess", &directory).await;
    await_data_access(&emulator, &directory, DISABLED).await;

    // A read, a write and an inventory listing: the identical refusal, because the gate is
    // on the directory.
    let read = emulator
        .data_plane(
            "/Users/DescribeUser",
            &directory,
            json!({"SAMAccountName": USER}),
        )
        .await;
    let write = emulator
        .data_plane(
            "/Users/CreateUser",
            &directory,
            json!({"SAMAccountName": REFUSED_USER, "GivenName": "Sim", "Surname": "Nope"}),
        )
        .await;
    let listing = emulator
        .data_plane("/Users/ListUsers", &directory, json!({}))
        .await;
    for (case, answer) in [
        ("DescribeUser while shut", &read),
        ("CreateUser while shut", &write),
        ("ListUsers while shut", &listing),
    ] {
        assert_data_disabled(case, answer);
    }
    assert_eq!(
        read.body, write.body,
        "a read and a write of a shut directory must carry the identical refusal, got {:?} \
         and {:?}",
        read.body, write.body
    );
    assert_eq!(
        read.body, listing.body,
        "a read and a listing of a shut directory must carry the identical refusal, got {:?} \
         and {:?}",
        read.body, listing.body
    );

    // The directory itself is untouched by the switch.
    assert_eq!(
        stage(&emulator, &directory).await,
        ACTIVE,
        "case the directory while its data plane is shut: expected Stage {ACTIVE}"
    );

    toggle(&emulator, "EnableDirectoryDataAccess", &directory).await;
    await_data_access(&emulator, &directory, ENABLED).await;

    // The same AD objects: the SIDs Active Directory minted cannot be re-derived, so a
    // cycle that lost the objects would answer with different ones.
    let user_after = emulator
        .data_plane(
            "/Users/DescribeUser",
            &directory,
            json!({"SAMAccountName": USER}),
        )
        .await;
    assert_eq!(
        identity(&user_after.body),
        identity(&user_before.body),
        "case the user across the cycle: expected the same AD object, got {:?} before and \
         {:?} after",
        user_before.body,
        user_after.body
    );
    let group_after = emulator
        .data_plane(
            "/Groups/DescribeGroup",
            &directory,
            json!({"SAMAccountName": GROUP}),
        )
        .await;
    assert_eq!(
        identity(&group_after.body),
        identity(&group_before.body),
        "case the group across the cycle: expected the same AD object, got {:?} before and \
         {:?} after",
        group_before.body,
        group_after.body
    );

    // The membership edge survives in both projections.
    let members = every_name(
        &emulator,
        "/GroupMemberships/ListGroupMembers",
        &directory,
        "Members",
        &json!({"SAMAccountName": GROUP}),
    )
    .await;
    assert!(
        members.iter().any(|member| member == USER),
        "case ListGroupMembers after the cycle: expected {USER:?} among {members:?}"
    );
    let memberships = every_name(
        &emulator,
        "/GroupMemberships/ListGroupsForMember",
        &directory,
        "Groups",
        &json!({"SAMAccountName": USER}),
    )
    .await;
    assert!(
        memberships.iter().any(|group| group == GROUP),
        "case ListGroupsForMember after the cycle: expected {GROUP:?} among {memberships:?}"
    );

    // The refused write created nothing, so the name it used is in no page of the inventory.
    let listed = every_name(&emulator, "/Users/ListUsers", &directory, "Users", &json!({})).await;
    assert!(
        listed.iter().any(|name| name == USER)
            && !listed.iter().any(|name| name == REFUSED_USER),
        "case the inventory after the cycle: expected {USER:?} and not {REFUSED_USER:?}, got \
         {listed:?}"
    );
}

/// The transient legs of the switch, which only the two planes together can show: an enable
/// answers before the data plane opens and a disable answers before it closes, and the data
/// plane's answer matches the status the control plane reports at the same instant.
#[tokio::test]
async fn the_data_plane_follows_the_status_the_control_plane_reports() {
    let emulator = Emulator::new();
    let created = emulator
        .control_plane(
            "CreateMicrosoftAD",
            create_microsoft_ad("transient.example.com"),
        )
        .await;
    let directory = created.text("DirectoryId");
    await_stage(&emulator, &directory, ACTIVE).await;

    // Straight after the enable the transition it owes is still outstanding, so the control
    // plane reports the transient status and the data plane is not open yet.
    toggle(&emulator, "EnableDirectoryDataAccess", &directory).await;
    let while_enabling = emulator
        .data_plane("/Users/ListUsers", &directory, json!({}))
        .await;
    assert_eq!(
        data_access_status(&emulator, &directory).await,
        ENABLING,
        "case straight after the enable: expected DataAccessStatus {ENABLING}"
    );
    assert_data_disabled("ListUsers while the enable is outstanding", &while_enabling);

    await_data_access(&emulator, &directory, ENABLED).await;
    let served = emulator
        .data_plane("/Users/ListUsers", &directory, json!({}))
        .await;
    assert_eq!(
        served.status, 200,
        "case once the enable has settled: expected the listing to be served, got {} with {:?}",
        served.status, served.body
    );

    // Straight after the disable the directory has not left the Data API yet, so the data
    // plane still answers — exactly as the control plane's transient status says.
    toggle(&emulator, "DisableDirectoryDataAccess", &directory).await;
    let while_disabling = emulator
        .data_plane("/Users/ListUsers", &directory, json!({}))
        .await;
    assert_eq!(
        data_access_status(&emulator, &directory).await,
        DISABLING,
        "case straight after the disable: expected DataAccessStatus {DISABLING}"
    );
    assert_eq!(
        while_disabling.status, 200,
        "case while the disable is outstanding: expected the listing to still be served, got \
         {} with {:?}",
        while_disabling.status, while_disabling.body
    );

    await_data_access(&emulator, &directory, DISABLED).await;
    let shut = emulator
        .data_plane("/Users/ListUsers", &directory, json!({}))
        .await;
    assert_data_disabled("ListUsers once the disable has settled", &shut);
}
