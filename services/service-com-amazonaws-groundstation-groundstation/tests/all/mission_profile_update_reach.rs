// SPDX-License-Identifier: CC0-1.0

//! How far an `UpdateMissionProfile` (`PUT /missionprofile/{missionProfileId}`) reaches,
//! end to end over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: "Updating a mission profile will not update the execution
//! parameters for existing future contacts" (`smithy_query(query="doc",
//! shape="UpdateMissionProfile")`). The dataflows of a pass are such a parameter, so an
//! update that REWIRES a profile's `dataflowEdges` leaves the pass of every contact
//! already reserved against it that is still to be flown on the wiring that contact was
//! reserved with, while the profile itself — and every contact reserved after the update —
//! carries the new wiring. A contact whose pass is over or whose reservation was released
//! has no future execution to protect and goes on reading the profile.
//!
//! The suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * the whole `ContactStatus` enum (the one table of it this target keeps,
//!   `crate::mission_profile_deletion::CONTACT_STATUSES`): a contact in each value in turn,
//!   reserved against a profile of its own, with the wiring kept for every non-terminal
//!   status and the profile's own read for every terminal one;
//! * the FORMS the rewiring can take — another edge, several edges, no edge at all, the
//!   same edge again, and an update that carries no `dataflowEdges` member — plus a profile
//!   rewired twice, where each contact keeps the wiring in force when IT was reserved;
//! * what the kept wiring still DEPENDS on: the dataflow endpoint group holding the
//!   endpoint a kept wiring selects is still in use by that contact, so its delete is
//!   refused until the contact is released.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_in_use_guard::group_holding;
use crate::dataflow_endpoint_groups::{delete_group, get_group};
use crate::minute_usage::{epoch, reserve_pass};
use crate::mission_profile_deletion::{put_in_status, CONTACT_STATUSES};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_with_engine, send, spectrum_config,
    stock_ground_stations, Reply, STATION,
};
use service_com_amazonaws_groundstation_groundstation::state::TERMINAL_CONTACT_STATUSES;

/// How long each reserved pass runs.
const PASS_SECONDS: i64 = 20 * 60;

/// How far apart two reserved windows sit. An antenna serves one pass at a time, so every
/// contact in a suite is reserved in a window of its own.
const WINDOW_STRIDE: i64 = 86_400;

/// One dataflow edge to wire a profile with: the two `ConfigArn`s it runs between, and the
/// name of the dataflow endpoint its destination selects.
struct Edge {
    source_arn: String,
    destination_arn: String,
    endpoint_name: String,
}

impl Edge {
    /// The edge as `dataflowEdges` carries it: a list of its two ARNs.
    fn wire(&self) -> Value {
        json!([self.source_arn, self.destination_arn])
    }
}

/// Create the two configs one dataflow edge runs between: an `antenna-downlink` source and
/// a `dataflowEndpointConfig` destination selecting the endpoint named `endpoint_name`.
///
/// The center frequency is derived from `label` only so each row's configs differ from the
/// next row's; what the edge is wired to is what these suites assert on.
async fn edge(router: &MultiServiceRouter, label: &str, endpoint_name: &str) -> Edge {
    let (_, _, source_arn) = create_config(
        router,
        &format!("{label} downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (_, _, destination_arn) = create_config(
        router,
        &format!("{label} endpoint"),
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": endpoint_name}}),
        None,
    )
    .await;
    Edge {
        source_arn,
        destination_arn,
        endpoint_name: endpoint_name.to_string(),
    }
}

/// `CreateMissionProfile` wired with `edges`, as `(missionProfileId, missionProfileArn)`.
async fn create_profile(
    router: &MultiServiceRouter,
    name: &str,
    edges: Vec<Value>,
) -> (String, String) {
    let tracking_arn = create_tracking_config(router, &format!("{name} tracking")).await;
    let created = send(
        router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": name,
            "minimumViableContactDurationSeconds": 60,
            "dataflowEdges": edges,
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup {name}: expected CreateMissionProfile 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");
    let read = read_profile(router, name, &id).await;
    (id, read.string("missionProfileArn"))
}

/// `GetMissionProfile`, asserting it answered.
async fn read_profile(router: &MultiServiceRouter, case: &str, profile_id: &str) -> Reply {
    let read = send(
        router,
        "GET",
        &format!("/missionprofile/{profile_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected GetMissionProfile 200, got {} {}",
        read.status, read.body
    );
    read
}

/// `UpdateMissionProfile` with `body`, asserting it was accepted.
async fn update_profile(router: &MultiServiceRouter, case: &str, profile_id: &str, body: Value) {
    let updated = send(
        router,
        "PUT",
        &format!("/missionprofile/{profile_id}"),
        Some(body),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected UpdateMissionProfile 200, got {} {}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.string("missionProfileId"),
        profile_id,
        "case {case}: the update must answer the profile it updated, got {}",
        updated.body
    );
}

/// The `dataflowList` a contact read reports, or `None` when it reports none at all.
async fn dataflows(router: &MultiServiceRouter, case: &str, contact_id: &str) -> Option<Value> {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    described.body.get("dataflowList").cloned()
}

/// The `dataflowList` `DescribeContactVersion` reports for the contact's FIRST version —
/// the other reader of a contact's dataflows, which must describe the same wiring.
async fn version_dataflows(
    router: &MultiServiceRouter,
    case: &str,
    contact_id: &str,
) -> Option<Value> {
    let described = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/1"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContactVersion 200, got {} {}",
        described.status, described.body
    );
    described.body.get("dataflowList").cloned()
}

#[tokio::test]
async fn an_update_that_rewires_a_profile_does_not_rewire_contacts_still_to_be_flown() {
    let (router, engine) = harness_with_engine();
    // The account is served by the station every pass below is booked at.
    stock_ground_stations(&engine, STATION);

    let first_window = epoch(2033, 5, 17, 9, 0);
    // Rows differ along the `ContactStatus` the contact holds when the update lands: the
    // rule is about the contacts that are still to be flown, which is every non-terminal
    // one, so the table spans the whole enum rather than the two statuses a reservation
    // reaches on its own.
    for (slot, status) in CONTACT_STATUSES.iter().enumerate() {
        let label = status.to_lowercase().replace('_', "-");
        // A profile, its configs and its contacts of their own per row, in windows of
        // their own, so one row's update cannot be what the next row observes.
        let reserved_edge =
            edge(&router, &format!("{label} reserved"), &format!("alpha-{label}")).await;
        let rewired_edge =
            edge(&router, &format!("{label} rewired"), &format!("beta-{label}")).await;
        let (profile_id, profile_arn) = create_profile(
            &router,
            &format!("update reach {label}"),
            vec![reserved_edge.wire()],
        )
        .await;

        let slot = i64::try_from(slot).expect("a row index fits in i64");
        let start = first_window + slot * 2 * WINDOW_STRIDE;
        let contact_id = reserve_pass(&router, &profile_arn, status, start, PASS_SECONDS).await;
        put_in_status(&engine, &contact_id, status);
        let before = dataflows(&router, status, &contact_id).await;
        assert_eq!(
            before.as_ref().and_then(Value::as_array).map(Vec::len),
            Some(1),
            "case {status}: the contact must report the one dataflow it was reserved with, \
             got {before:?}"
        );

        update_profile(
            &router,
            status,
            &profile_id,
            json!({"dataflowEdges": [rewired_edge.wire()]}),
        )
        .await;

        // The update landed on the profile itself: the profile reports the new wiring.
        let profile = read_profile(&router, status, &profile_id).await;
        assert_eq!(
            profile.field("dataflowEdges"),
            &json!([rewired_edge.wire()]),
            "case {status}: the updated profile must report the wiring the update carried, \
             got {}",
            profile.body
        );
        // And a contact reserved AFTER it is flown through that new wiring — the reference
        // for what the kept wiring must NOT have become.
        let fresh_id = reserve_pass(
            &router,
            &profile_arn,
            &format!("{status} fresh"),
            start + WINDOW_STRIDE,
            PASS_SECONDS,
        )
        .await;
        let rewired = dataflows(&router, status, &fresh_id).await;
        assert_ne!(
            rewired, before,
            "case {status}: the rewiring must be observable at all, got {rewired:?}"
        );

        let after = dataflows(&router, status, &contact_id).await;
        let version_after = version_dataflows(&router, status, &contact_id).await;
        if TERMINAL_CONTACT_STATUSES.contains(status) {
            assert_eq!(
                after, rewired,
                "case {status}: a contact with no pass left to fly reads its profile, so it \
                 must report the profile's new wiring, got {after:?}"
            );
        } else {
            assert_eq!(
                after, before,
                "case {status}: a contact still to be flown must keep the wiring it was \
                 reserved with, got {after:?}"
            );
        }
        assert_eq!(
            version_after, after,
            "case {status}: both readers of a contact's dataflows must describe one wiring, \
             got {version_after:?}"
        );
    }
}

/// One form an update's rewiring can take: the `dataflowEdges` member it carries (`None`
/// for an update that carries none at all) and how many dataflows the profile holds
/// afterwards.
struct Rewiring {
    case: &'static str,
    /// Built from the row's own edges, so each form is wired to configs that exist.
    edges: fn(&Edge, &Edge) -> Option<Vec<Value>>,
    /// How many dataflows a contact reserved AFTER this update reports.
    fresh_dataflows: usize,
}

#[tokio::test]
async fn a_contact_keeps_the_wiring_in_force_when_it_was_reserved_whatever_form_the_update_takes() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    // Rows differ along WHAT the update does to the wiring: points it at another edge,
    // widens it to several edges, empties it, carries the wiring the profile already holds,
    // or carries no `dataflowEdges` member at all and only renames the profile. The member
    // is `smithy_query(query="operation", operation="UpdateMissionProfile")`'s
    // `dataflowEdges` (DataflowEdgeList, len 0..500), so an empty list is a value it takes.
    let rows = [
        Rewiring {
            case: "pointed at another edge",
            edges: |_, rewired| Some(vec![rewired.wire()]),
            fresh_dataflows: 1,
        },
        Rewiring {
            case: "widened to two edges",
            edges: |reserved, rewired| Some(vec![reserved.wire(), rewired.wire()]),
            fresh_dataflows: 2,
        },
        Rewiring {
            case: "emptied",
            edges: |_, _| Some(Vec::new()),
            fresh_dataflows: 0,
        },
        Rewiring {
            case: "carrying the wiring the profile already holds",
            edges: |reserved, _| Some(vec![reserved.wire()]),
            fresh_dataflows: 1,
        },
        Rewiring {
            case: "carrying no dataflowEdges member at all",
            edges: |_, _| None,
            fresh_dataflows: 1,
        },
    ];

    let first_window = epoch(2034, 2, 3, 11, 0);
    for (slot, row) in rows.iter().enumerate() {
        let label = row.case.replace(' ', "-");
        let reserved_edge =
            edge(&router, &format!("{label} reserved"), &format!("a-{slot}")).await;
        let rewired_edge =
            edge(&router, &format!("{label} rewired"), &format!("b-{slot}")).await;
        let (profile_id, profile_arn) = create_profile(
            &router,
            &format!("update form {slot}"),
            vec![reserved_edge.wire()],
        )
        .await;

        let slot = i64::try_from(slot).expect("a row index fits in i64");
        let start = first_window + slot * 2 * WINDOW_STRIDE;
        let contact_id = reserve_pass(&router, &profile_arn, row.case, start, PASS_SECONDS).await;
        let before = dataflows(&router, row.case, &contact_id).await;

        let carried = (row.edges)(&reserved_edge, &rewired_edge);
        let mut body = json!({"name": format!("renamed {slot}")});
        if let Some(edges) = carried.clone() {
            body["dataflowEdges"] = Value::Array(edges);
        }
        update_profile(&router, row.case, &profile_id, body).await;

        let after = dataflows(&router, row.case, &contact_id).await;
        assert_eq!(
            after, before,
            "case {}: a contact still to be flown must keep the wiring it was reserved with, \
             got {after:?}",
            row.case
        );

        // The profile itself took the update, and a contact reserved after it is flown
        // through what the profile now holds.
        let profile = read_profile(&router, row.case, &profile_id).await;
        let expected_edges = carried.unwrap_or_else(|| vec![reserved_edge.wire()]);
        assert_eq!(
            profile.field("dataflowEdges"),
            &Value::Array(expected_edges),
            "case {}: the updated profile must report the wiring in force for it, got {}",
            row.case,
            profile.body
        );
        let fresh_id = reserve_pass(
            &router,
            &profile_arn,
            row.case,
            start + WINDOW_STRIDE,
            PASS_SECONDS,
        )
        .await;
        let fresh = dataflows(&router, row.case, &fresh_id).await;
        assert_eq!(
            fresh.as_ref().and_then(Value::as_array).map(Vec::len),
            Some(row.fresh_dataflows),
            "case {}: a contact reserved after the update must be flown through the {} \
             dataflows the profile now holds, got {fresh:?}",
            row.case,
            row.fresh_dataflows
        );
    }
}

#[tokio::test]
async fn each_contact_of_a_twice_rewired_profile_keeps_the_wiring_of_its_own_reservation() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    let first = edge(&router, "first", "endpoint-first").await;
    let second = edge(&router, "second", "endpoint-second").await;
    let (profile_id, profile_arn) =
        create_profile(&router, "twice rewired", vec![first.wire()]).await;

    let window = epoch(2035, 7, 9, 13, 30);
    // One contact reserved under each wiring the profile holds in turn, so each row's
    // expectation is the wiring that was in force when IT was reserved.
    let before_any = reserve_pass(&router, &profile_arn, "before-any", window, PASS_SECONDS).await;
    let reserved_first = dataflows(&router, "before-any", &before_any).await;

    update_profile(
        &router,
        "first rewiring",
        &profile_id,
        json!({"dataflowEdges": [second.wire()]}),
    )
    .await;
    let between = reserve_pass(
        &router,
        &profile_arn,
        "between",
        window + WINDOW_STRIDE,
        PASS_SECONDS,
    )
    .await;
    let reserved_second = dataflows(&router, "between", &between).await;
    assert_ne!(
        reserved_second, reserved_first,
        "case between: the first rewiring must be observable at all, got {reserved_second:?}"
    );

    update_profile(
        &router,
        "second rewiring",
        &profile_id,
        json!({"dataflowEdges": []}),
    )
    .await;
    let after_all = reserve_pass(
        &router,
        &profile_arn,
        "after-all",
        window + 2 * WINDOW_STRIDE,
        PASS_SECONDS,
    )
    .await;

    for (case, contact_id, expected) in [
        ("before-any", &before_any, reserved_first.clone()),
        ("between", &between, reserved_second.clone()),
        ("after-all", &after_all, Some(json!([]))),
    ] {
        let after = dataflows(&router, case, contact_id).await;
        assert_eq!(
            after, expected,
            "case {case}: the contact must be flown through the wiring in force when it was \
             reserved, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_group_holding_an_endpoint_a_kept_wiring_selects_is_still_in_use_by_that_contact() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);

    // Two groups, each holding the endpoint one of the two wirings selects: a dataflow
    // graph is flown through the endpoints of ONE group, so the wiring the contact keeps
    // and the wiring the profile moves to each have a group of their own.
    let reserved_group = group_holding(&router, "reserved", "endpoint", &["kept-endpoint"]).await;
    let rewired_group = group_holding(&router, "rewired", "endpoint", &["new-endpoint"]).await;
    let reserved_edge = edge(&router, "kept", "kept-endpoint").await;
    let rewired_edge = edge(&router, "new", "new-endpoint").await;
    let (profile_id, profile_arn) =
        create_profile(&router, "endpoint dependency", vec![reserved_edge.wire()]).await;

    let window = epoch(2036, 1, 20, 8, 0);
    let contact_id = reserve_pass(&router, &profile_arn, "kept", window, PASS_SECONDS).await;
    update_profile(
        &router,
        "kept",
        &profile_id,
        json!({"dataflowEdges": [rewired_edge.wire()]}),
    )
    .await;

    // The contact is still to be flown through the endpoint its kept wiring selects, so the
    // group holding that endpoint is still in use and its delete is refused naming the
    // contact — the in-use question and the contact's own read answer one wiring.
    let refused = delete_group(&router, &reserved_group).await;
    assert_eq!(
        refused.status, 431,
        "case kept-endpoint: the group holding the endpoint the kept wiring selects must \
         still be in use, got {} {}",
        refused.status, refused.body
    );
    assert!(
        refused.string("message").contains(&contact_id),
        "case kept-endpoint: the refusal must name the contact that is still wired to it, \
         got {}",
        refused.body
    );
    assert_eq!(
        get_group(&router, &reserved_group).await.status,
        200,
        "case kept-endpoint: a refused delete must leave the group readable"
    );
    // The group the profile was rewired TO holds an endpoint no contact is wired to yet,
    // since the only contact of this profile keeps the wiring it was reserved with.
    let endpoint_names = [&reserved_edge.endpoint_name, &rewired_edge.endpoint_name];
    assert_ne!(
        endpoint_names[0], endpoint_names[1],
        "case setup: the two wirings must select different endpoints, got {endpoint_names:?}"
    );
    let dropped = delete_group(&router, &rewired_group).await;
    assert_eq!(
        dropped.status, 200,
        "case new-endpoint: a group no contact is wired to yet must delete, got {} {}",
        dropped.status, dropped.body
    );

    // Released, the contact has no pass left to fly: nothing is wired to the kept endpoint
    // any more and the same delete lands.
    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case release: expected CancelContact 200, got {} {}",
        cancelled.status, cancelled.body
    );
    let deleted = delete_group(&router, &reserved_group).await;
    assert_eq!(
        deleted.status, 200,
        "case release: with its only holder released the delete must land, got {} {}",
        deleted.status, deleted.body
    );
}
