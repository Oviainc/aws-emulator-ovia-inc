// SPDX-License-Identifier: CC0-1.0

//! The status window an update opens, end to end over the router: `UpdateMember` and
//! `UpdateNode` START a configuration change, so the record they write reports
//! `UPDATING` ("The member is in the process of being updated" — `Member$Status`; "The
//! node is in the process of being updated" — `Node$Status`) until the clock owes it
//! `AVAILABLE` again, and the listings' `status` filter selects on that same stored
//! value.
//!
//! The tables below span the class the rule generalises over: WHICH resource is updated
//! (member, node) crossed with WHAT the request names (a log stream switched on, switched
//! off, a nested structure naming no parameter at all, and no configuration at all), and
//! the status the record was in when the update arrived (in service, a creation still
//! running, a removal already under way). The `Status` member names and their enum
//! values come from `smithy_query` (`Member$Status`, `Node$Status`,
//! `ListMembersInput$Status` / `ListNodesInput$Status`, both bound as the `status`
//! `httpQuery`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    available_network, call, create_network, create_network_body, delete_member, get_member,
    get_node, listed_ids, listed_member_ids, new_router, Answer, PAST_TRANSITION,
};
use crate::network_and_node_lifecycle::create_node;

/// The `MemberStatus` / `NodeStatus` value a record in service reports.
const AVAILABLE: &str = "AVAILABLE";

/// The `MemberStatus` / `NodeStatus` value a record reports while a configuration change
/// it is in runs.
const UPDATING: &str = "UPDATING";

/// The instance type every node in these cases runs on (one the `STANDARD` edition
/// offers).
const INSTANCE_TYPE: &str = "bc.t3.small";

/// A `MemberLogPublishingConfiguration` naming the member's CA log stream with the
/// CloudWatch `Enabled` given, or an empty `Fabric` structure when it names none.
fn member_logs(ca: Option<bool>) -> Value {
    match ca {
        Some(enabled) => json!({ "Fabric": { "CaLogs": { "Cloudwatch": { "Enabled": enabled } } } }),
        None => json!({ "Fabric": {} }),
    }
}

/// A `NodeLogPublishingConfiguration` naming the node's peer log stream with the
/// CloudWatch `Enabled` given, or an empty `Fabric` structure when it names none.
fn node_logs(peer: Option<bool>) -> Value {
    match peer {
        Some(enabled) => {
            json!({ "Fabric": { "PeerLogs": { "Cloudwatch": { "Enabled": enabled } } } })
        }
        None => json!({ "Fabric": {} }),
    }
}

/// One `UpdateMember` call: `PATCH /networks/{NetworkId}/members/{MemberId}`, naming the
/// configuration given (and nothing at all when it is `None`).
async fn update_member(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    patch: Option<&Value>,
) -> Answer {
    let mut body = json!({});
    if let Some(patch) = patch {
        body["LogPublishingConfiguration"] = patch.clone();
    }
    call(
        router,
        "PATCH",
        &format!("/networks/{network_id}/members/{member_id}"),
        Some(body),
    )
    .await
}

/// One `UpdateNode` call: `PATCH /networks/{NetworkId}/nodes/{NodeId}`, scoped by the
/// member that owns the node (`UpdateNodeInput$MemberId` rides in the body).
async fn update_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
    patch: Option<&Value>,
) -> Answer {
    let mut body = json!({ "MemberId": member_id });
    if let Some(patch) = patch {
        body["LogPublishingConfiguration"] = patch.clone();
    }
    call(
        router,
        "PATCH",
        &format!("/networks/{network_id}/nodes/{node_id}"),
        Some(body),
    )
    .await
}

/// The `Status` `GetMember` reports for one member.
async fn member_status(router: &MultiServiceRouter, network_id: &str, member_id: &str) -> String {
    let answer = get_member(router, network_id, member_id).await;
    assert_eq!(
        answer.status, 200,
        "case GetMember {member_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["Member", "Status"])
}

/// The `Status` `GetNode` reports for one node.
async fn node_status(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> String {
    let answer = get_node(router, network_id, member_id, node_id).await;
    assert_eq!(
        answer.status, 200,
        "case GetNode {node_id}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["Node", "Status"])
}

/// The node ids `ListNodes` reports for `network_id` in `status`.
async fn listed_node_ids(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    status: &str,
) -> Vec<String> {
    let answer = call(
        router,
        "GET",
        &format!("/networks/{network_id}/nodes?memberId={member_id}&status={status}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case ListNodes status={status}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    listed_ids(&answer, "Nodes")
}

/// One row: what the update names, and whether that opens the update window.
#[derive(Debug)]
struct Case {
    name: &'static str,
    patch: Option<Value>,
    /// The status the record must report straight after the update.
    during: &'static str,
}

/// The rows every update table shares: a request that names a configuration to apply
/// starts the change, one that names none asks for no change and starts none.
fn cases(logs: fn(Option<bool>) -> Value) -> Vec<Case> {
    vec![
        Case {
            name: "a log stream switched on",
            patch: Some(logs(Some(true))),
            during: UPDATING,
        },
        Case {
            name: "a log stream switched off",
            patch: Some(logs(Some(false))),
            during: UPDATING,
        },
        Case {
            name: "a configuration naming no parameter",
            patch: Some(logs(None)),
            during: UPDATING,
        },
        Case {
            name: "an empty configuration",
            patch: Some(json!({})),
            during: UPDATING,
        },
        Case {
            name: "no configuration at all",
            patch: None,
            during: AVAILABLE,
        },
    ]
}

/// `UpdateMember` reports the member `UPDATING` until the clock settles it back to
/// `AVAILABLE`, and `ListMembers` selects on that same stored status.
#[tokio::test]
async fn update_member_reports_the_member_updating_until_the_change_settles() {
    for (index, case) in cases(member_logs).iter().enumerate() {
        let router = new_router();
        let (network_id, member_id) =
            available_network(&router, &format!("upd-window-member-{index}"), "net-upd-member")
                .await;

        let before = member_status(&router, &network_id, &member_id).await;
        assert_eq!(
            before, AVAILABLE,
            "case {}: expected the member to be {AVAILABLE} before the update, got \
             {before:?}",
            case.name
        );

        let answer = update_member(&router, &network_id, &member_id, case.patch.as_ref()).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected UpdateMember 200, got {} {:?}",
            case.name, answer.status, answer.body
        );

        // Straight after the call, with nothing slept: the window is non-zero, so a
        // caller acting immediately still observes the change under way.
        let during = member_status(&router, &network_id, &member_id).await;
        assert_eq!(
            during, case.during,
            "case {}: expected the member to be {:?} straight after the update, got \
             {during:?}",
            case.name, case.during
        );

        // The listing filter selects on the stored status: the member is reported under
        // the status it is in and under no other.
        let updating = listed_member_ids(&router, &network_id, "status=UPDATING").await;
        let available = listed_member_ids(&router, &network_id, "status=AVAILABLE").await;
        let (expected_updating, expected_available) = if case.during == UPDATING {
            (vec![member_id.clone()], Vec::new())
        } else {
            (Vec::new(), vec![member_id.clone()])
        };
        assert_eq!(
            updating, expected_updating,
            "case {}: expected ListMembers status=UPDATING to report \
             {expected_updating:?}, got {updating:?}",
            case.name
        );
        assert_eq!(
            available, expected_available,
            "case {}: expected ListMembers status=AVAILABLE to report \
             {expected_available:?}, got {available:?}",
            case.name
        );

        // The change settles on its own: the clock owes the member `AVAILABLE` again.
        tokio::time::sleep(PAST_TRANSITION).await;
        let after = member_status(&router, &network_id, &member_id).await;
        assert_eq!(
            after, AVAILABLE,
            "case {}: expected the member to settle back to {AVAILABLE}, got {after:?}",
            case.name
        );
        let settled = listed_member_ids(&router, &network_id, "status=AVAILABLE").await;
        assert_eq!(
            settled,
            vec![member_id.clone()],
            "case {}: expected ListMembers status=AVAILABLE to report the settled member, \
             got {settled:?}",
            case.name
        );

        // And the configuration the update named is the one the settled member carries,
        // so the window reports a change that really was applied. The member was created
        // naming no configuration, so what it carries afterwards is exactly what the
        // update named (and nothing at all when the update named none).
        let expected = case.patch.clone().unwrap_or(Value::Null);
        let answer = get_member(&router, &network_id, &member_id).await;
        let stored = answer.body["Member"]["LogPublishingConfiguration"].clone();
        assert_eq!(
            stored, expected,
            "case {}: expected the settled member to carry {expected:?}, got {stored:?}",
            case.name
        );
    }
}

/// `UpdateNode` reports the node `UPDATING` until the clock settles it back to
/// `AVAILABLE`, and `ListNodes` selects on that same stored status — the node-side twin
/// of the member case above.
#[tokio::test]
async fn update_node_reports_the_node_updating_until_the_change_settles() {
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "upd-window-node", "net-upd-node").await;

    for (index, case) in cases(node_logs).iter().enumerate() {
        let node_id = create_node(
            &router,
            &network_id,
            &member_id,
            &format!("upd-window-node-{index}"),
            json!({ "InstanceType": INSTANCE_TYPE }),
        )
        .await;
        // A node starts out `CREATING`; the update window is what an update opens on one
        // already in service.
        tokio::time::sleep(PAST_TRANSITION).await;
        let before = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            before, AVAILABLE,
            "case {}: expected the node to be {AVAILABLE} before the update, got {before:?}",
            case.name
        );

        let answer =
            update_node(&router, &network_id, &member_id, &node_id, case.patch.as_ref()).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected UpdateNode 200, got {} {:?}",
            case.name, answer.status, answer.body
        );

        let during = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            during, case.during,
            "case {}: expected the node to be {:?} straight after the update, got {during:?}",
            case.name, case.during
        );
        let updating = listed_node_ids(&router, &network_id, &member_id, UPDATING).await;
        let expected_updating: Vec<String> = if case.during == UPDATING {
            vec![node_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            updating, expected_updating,
            "case {}: expected ListNodes status=UPDATING to report {expected_updating:?}, \
             got {updating:?}",
            case.name
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let after = node_status(&router, &network_id, &member_id, &node_id).await;
        assert_eq!(
            after, AVAILABLE,
            "case {}: expected the node to settle back to {AVAILABLE}, got {after:?}",
            case.name
        );
        let settled = listed_node_ids(&router, &network_id, &member_id, AVAILABLE).await;
        assert!(
            settled.contains(&node_id),
            "case {}: expected ListNodes status=AVAILABLE to report the settled node \
             {node_id:?}, got {settled:?}",
            case.name
        );

        // The node was created naming no configuration, so what it carries after the
        // window closes is exactly what the update named.
        let expected = case.patch.clone().unwrap_or(Value::Null);
        let answer = get_node(&router, &network_id, &member_id, &node_id).await;
        let stored = answer.body["Node"]["LogPublishingConfiguration"].clone();
        assert_eq!(
            stored, expected,
            "case {}: expected the settled node to carry {expected:?}, got {stored:?}",
            case.name
        );
    }
}

/// The window is opened by each update, not once: an update made after the previous one
/// settled opens it again.
#[tokio::test]
async fn each_update_opens_the_window_again() {
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "upd-window-repeat", "net-upd-repeat").await;

    // Rows: one setting at a time, so each step both changes the record and re-opens the
    // window the previous step's settle closed.
    let steps: Vec<(&str, Value)> = vec![
        ("switch the CA log stream on", member_logs(Some(true))),
        ("switch it back off", member_logs(Some(false))),
        ("switch it on again", member_logs(Some(true))),
    ];

    for (case, patch) in &steps {
        let answer = update_member(&router, &network_id, &member_id, Some(patch)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateMember 200, got {} {:?}",
            answer.status, answer.body
        );
        let during = member_status(&router, &network_id, &member_id).await;
        assert_eq!(
            during, UPDATING,
            "case {case}: expected the member to be {UPDATING} straight after the update, \
             got {during:?}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;
        let after = member_status(&router, &network_id, &member_id).await;
        assert_eq!(
            after, AVAILABLE,
            "case {case}: expected the member to settle back to {AVAILABLE}, got {after:?}"
        );
        let answer = get_member(&router, &network_id, &member_id).await;
        let stored = answer.body["Member"]["LogPublishingConfiguration"].clone();
        assert_eq!(
            stored, *patch,
            "case {case}: expected the settled member to carry {patch:?}, got {stored:?}"
        );
    }
}

/// An update that arrives while the previous one's window is still open keeps the record
/// `UPDATING` and applies its own parameters: each update starts a change of its own, so
/// the window runs from the call that asked for it.
#[tokio::test]
async fn a_second_update_inside_the_window_keeps_it_open() {
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "upd-window-overlap", "net-upd-overlap").await;

    // Rows: the second update names the opposite setting, so what the record carries
    // afterwards says which call's parameters won.
    let steps: Vec<(&str, Value)> = vec![
        ("the first update", member_logs(Some(true))),
        ("a second update inside the window", member_logs(Some(false))),
    ];
    for (case, patch) in &steps {
        let answer = update_member(&router, &network_id, &member_id, Some(patch)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateMember 200, got {} {:?}",
            answer.status, answer.body
        );
        let during = member_status(&router, &network_id, &member_id).await;
        assert_eq!(
            during, UPDATING,
            "case {case}: expected the member to be {UPDATING} while the change runs, got \
             {during:?}"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;
    let after = member_status(&router, &network_id, &member_id).await;
    assert_eq!(
        after, AVAILABLE,
        "case after both updates: expected the member to settle back to {AVAILABLE}, got \
         {after:?}"
    );
    let answer = get_member(&router, &network_id, &member_id).await;
    let stored = answer.body["Member"]["LogPublishingConfiguration"].clone();
    assert_eq!(
        stored,
        member_logs(Some(false)),
        "case after both updates: expected the second update's parameters to be the ones \
         the member carries, got {stored:?}"
    );
}

/// An update does not take a record out of a transition it is already in: a creation
/// still running keeps reporting `CREATING`, and a removal already under way keeps
/// reporting `DELETING` — the update window belongs to a record that is in service.
#[tokio::test]
async fn an_update_does_not_disturb_a_transition_already_under_way() {
    // A member whose creation is still running: the update applies its parameters and the
    // creation keeps running to `AVAILABLE`.
    let router = new_router();
    let (network_id, member_id) = create_network(
        &router,
        create_network_body("upd-window-creating", "net-upd-creating"),
    )
    .await;
    let answer = update_member(
        &router,
        &network_id,
        &member_id,
        Some(&member_logs(Some(true))),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case a member still creating: expected UpdateMember 200, got {} {:?}",
        answer.status, answer.body
    );
    let during = member_status(&router, &network_id, &member_id).await;
    assert_eq!(
        during, "CREATING",
        "case a member still creating: expected the creation to keep reporting CREATING, \
         got {during:?}"
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    let after = member_status(&router, &network_id, &member_id).await;
    assert_eq!(
        after, AVAILABLE,
        "case a member still creating: expected the creation to settle to {AVAILABLE}, got \
         {after:?}"
    );
    let answer = get_member(&router, &network_id, &member_id).await;
    let stored = answer.body["Member"]["LogPublishingConfiguration"].clone();
    assert_eq!(
        stored,
        member_logs(Some(true)),
        "case a member still creating: expected the update's parameters to be applied, got \
         {stored:?}"
    );

    // A member whose removal has begun: nothing brings it back out of `DELETING`.
    let router = new_router();
    let (network_id, member_id) =
        available_network(&router, "upd-window-deleting", "net-upd-deleting").await;
    let answer = delete_member(&router, &network_id, &member_id).await;
    assert_eq!(
        answer.status, 200,
        "case a member being removed: expected DeleteMember 200, got {} {:?}",
        answer.status, answer.body
    );
    let answer = update_member(
        &router,
        &network_id,
        &member_id,
        Some(&member_logs(Some(true))),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case a member being removed: expected UpdateMember 200, got {} {:?}",
        answer.status, answer.body
    );
    let during = member_status(&router, &network_id, &member_id).await;
    assert_eq!(
        during, "DELETING",
        "case a member being removed: expected the removal to keep reporting DELETING, got \
         {during:?}"
    );
    let updating = listed_member_ids(&router, &network_id, "status=UPDATING").await;
    assert!(
        updating.is_empty(),
        "case a member being removed: expected ListMembers status=UPDATING to report \
         nothing, got {updating:?}"
    );
}
