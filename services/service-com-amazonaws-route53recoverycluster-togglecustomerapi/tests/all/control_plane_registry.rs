// SPDX-License-Identifier: CC0-1.0

//! The data plane serves the CONTROL plane's resources: what
//! `route53-recovery-control-config` created, and nothing else.
//!
//! Route 53 ARC splits one resource namespace across two APIs — the config (control)
//! plane creates clusters, control panels and routing controls; this cluster (data)
//! plane only reads and toggles them — so every case here creates its resources through
//! the control plane's own API and then addresses them through the data plane's, over
//! the wire, on one emulator.

use serde_json::{json, Value};

use super::common::{
    call, control_plane, create_cluster, create_control_panel, create_routing_control,
    delete_control_panel, delete_routing_control, emulator, encode_arn, error_code, error_message,
    get_state, list_all_pages, listed_arns, unknown_arn_like,
};

/// RULE: a routing control is addressable by the data plane exactly while the control
/// plane holds it — `GetRoutingControlState`, `UpdateRoutingControlState` and
/// `ListRoutingControls` serve one the control plane created, and answer
/// `ResourceNotFoundException` (404) for one it deleted.
///
/// The rows differ in the control panel a control is created in (the panel the cluster
/// was created with, which the request names implicitly, and a panel the caller created),
/// in the cluster hosting it, and in how many controls the panel holds.
#[tokio::test]
async fn a_control_plane_create_is_what_the_data_plane_serves() {
    let router = emulator();

    // Row 1: a routing control created with no `ControlPanelArn` at all, which the
    // control plane attaches to the panel the cluster was created with.
    let first_cluster = create_cluster(&router, "registry-one").await;
    let (status, created) = control_plane(
        &router,
        "POST",
        "/routingcontrol",
        Some(json!({ "ClusterArn": first_cluster, "RoutingControlName": "default-panel-control" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the cluster's own control panel: expected CreateRoutingControl 200, got {status} with {created}"
    );
    let default_panel_control = created["RoutingControl"]["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let default_panel = created["RoutingControl"]["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows 2..: control panels the caller created, on two different clusters, holding
    // one and two routing controls.
    let second_cluster = create_cluster(&router, "registry-two").await;
    let mut created_controls: Vec<(String, String, String)> = vec![(
        "the cluster's own control panel".to_string(),
        default_panel_control.clone(),
        default_panel.clone(),
    )];
    let mut panels = vec![(default_panel.clone(), vec![default_panel_control.clone()])];
    for (case, cluster_arn, panel_name, control_names) in [
        (
            "a caller-created panel holding one control",
            first_cluster.as_str(),
            "registry-one-panel",
            vec!["lone-control"],
        ),
        (
            "a caller-created panel on another cluster holding two controls",
            second_cluster.as_str(),
            "registry-two-panel",
            vec!["first-control", "second-control"],
        ),
    ] {
        let panel = create_control_panel(&router, cluster_arn, panel_name).await;
        let mut hosted = Vec::new();
        for name in control_names {
            let control = create_routing_control(&router, &panel, name).await;
            created_controls.push((case.to_string(), control.arn.clone(), panel.arn.clone()));
            hosted.push(control.arn);
        }
        panels.push((panel.arn.clone(), hosted));
    }

    // Every created control is served by the data plane, with the state a new routing
    // control has, and is toggled through it.
    for (case, arn, panel_arn) in &created_controls {
        let read = get_state(&router, arn).await;
        assert_eq!(
            read.get("RoutingControlState").and_then(Value::as_str),
            Some("Off"),
            "case {case}: a created routing control starts Off, got {read}"
        );

        let (status, body) = call(
            &router,
            "UpdateRoutingControlState",
            &json!({ "RoutingControlArn": arn, "RoutingControlState": "On" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateRoutingControlState 200, got {status} with {body}"
        );
        let read_back = get_state(&router, arn).await;
        assert_eq!(
            read_back.get("RoutingControlState").and_then(Value::as_str),
            Some("On"),
            "case {case}: the toggle must be what a later read reports, got {read_back}"
        );

        // The listing filtered by the hosting panel reports it too.
        let listed = listed_arns(&list_all_pages(&router, Some(panel_arn), None, 8).await);
        assert!(
            listed.contains(arn),
            "case {case}: {panel_arn} must list {arn}, got {listed:?}"
        );
    }

    // A control created LATER shows up without anything re-seeding the data plane: the
    // registry is read per request, not snapshotted.
    let late_panel = create_control_panel(&router, &second_cluster, "registry-late-panel").await;
    let late = create_routing_control(&router, &late_panel, "late-control").await;
    let listed = listed_arns(&list_all_pages(&router, None, None, 8).await);
    assert!(
        listed.contains(&late.arn),
        "case a control created after the first listing: it must be listed, got {listed:?}"
    );

    // And a control the control plane DELETES stops being addressable, state and all.
    delete_routing_control(&router, &late.arn).await;
    for (operation, body) in [
        (
            "GetRoutingControlState",
            json!({ "RoutingControlArn": late.arn }),
        ),
        (
            "UpdateRoutingControlState",
            json!({ "RoutingControlArn": late.arn, "RoutingControlState": "On" }),
        ),
    ] {
        let (status, answered) = call(&router, operation, &body).await;
        assert_eq!(
            status, 404,
            "case a deleted routing control via {operation}: expected 404, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case a deleted routing control via {operation}: expected ResourceNotFoundException, got {answered}"
        );
    }
    let listed = listed_arns(&list_all_pages(&router, None, None, 8).await);
    assert!(
        !listed.contains(&late.arn),
        "case a deleted routing control: it must not be listed, got {listed:?}"
    );

    // A control panel the control plane deletes stops being a listable filter.
    delete_control_panel(&router, &late_panel.arn).await;
    let (status, body) = call(
        &router,
        "ListRoutingControls",
        &json!({ "ControlPanelArn": late_panel.arn }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case a deleted control panel: expected 404, got {status} with {body}"
    );

    // Nothing the data plane did changed what the control plane reports.
    let (status, described) = control_plane(
        &router,
        "GET",
        &format!("/routingcontrol/{}", encode_arn(&default_panel_control)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case DescribeRoutingControl after toggling: expected 200, got {status} with {described}"
    );
    assert_eq!(
        described["RoutingControl"]["RoutingControlArn"]
            .as_str()
            .unwrap_or_default(),
        default_panel_control,
        "case DescribeRoutingControl after toggling: the control plane still holds it, got {described}"
    );
}

/// RULE: the data plane serves ONLY what the control plane holds — a well-formed ARN no
/// create minted is `ResourceNotFoundException` (404), worded as Route 53 ARC words it:
/// `Routing Control '<arn>' not found` for a `RoutingControlArn`, and
/// `Control panel not found: '<arn>'` for a `ControlPanelArn` filter.
///
/// The rows differ in how the unknown ARN is formed: one hex digit of a real id changed,
/// a fresh id of the same shape, a resource under a panel that does not exist, and an
/// ARN of another resource kind.
#[tokio::test]
async fn an_arn_no_create_minted_is_not_found_with_the_services_own_wording() {
    let router = emulator();
    let cluster = create_cluster(&router, "wording").await;
    let panel = create_control_panel(&router, &cluster, "wording-panel").await;
    let control = create_routing_control(&router, &panel, "wording-control").await;

    // An id of the shape the control plane mints (8-4-4-4-12 hexadecimal) that no
    // create produced.
    let fresh_id = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";
    let unknown_controls = [
        (
            "one hex digit of a real routing control id changed",
            unknown_arn_like(&control.arn),
        ),
        (
            "a fresh routing control id under a real panel",
            format!("{}/routingcontrol/{fresh_id}", panel.arn),
        ),
        (
            "a routing control under a panel that does not exist",
            format!("{}/routingcontrol/{fresh_id}", unknown_arn_like(&panel.arn)),
        ),
        ("a cluster ARN where a routing control ARN belongs", cluster),
    ];

    for (case, arn) in &unknown_controls {
        for operation in ["GetRoutingControlState", "UpdateRoutingControlState"] {
            let mut body = serde_json::Map::new();
            body.insert("RoutingControlArn".to_string(), json!(arn));
            if operation == "UpdateRoutingControlState" {
                body.insert("RoutingControlState".to_string(), json!("On"));
            }
            let (status, answered) = call(&router, operation, &Value::Object(body)).await;
            assert_eq!(
                status, 404,
                "case {case} via {operation}: expected 404, got {status} with {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case} via {operation}: expected ResourceNotFoundException, got {answered}"
            );
            assert_eq!(
                error_message(&answered),
                format!("Routing Control '{arn}' not found"),
                "case {case} via {operation}: expected the service's own wording, got {answered}"
            );
            assert_eq!(
                answered.get("resourceId").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case} via {operation}: the refusal names the ARN asked for, got {answered}"
            );
        }
    }

    // A batch update refuses an unknown entry the same way.
    let (status, answered) = call(
        &router,
        "UpdateRoutingControlStates",
        &json!({ "UpdateRoutingControlStateEntries": [
            { "RoutingControlArn": unknown_arn_like(&control.arn), "RoutingControlState": "On" }
        ]}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case an unknown entry in a batch: expected 404, got {status} with {answered}"
    );
    assert_eq!(
        error_message(&answered),
        format!("Routing Control '{}' not found", unknown_arn_like(&control.arn)),
        "case an unknown entry in a batch: expected the service's own wording, got {answered}"
    );

    let unknown_panels = [
        (
            "one hex digit of a real control panel id changed",
            unknown_arn_like(&panel.arn),
        ),
        (
            "a fresh control panel id",
            format!(
                "{}{fresh_id}",
                panel
                    .arn
                    .rsplit_once('/')
                    .map(|(prefix, _)| format!("{prefix}/"))
                    .unwrap_or_default()
            ),
        ),
        ("a routing control ARN where a panel ARN belongs", control.arn),
    ];

    for (case, arn) in &unknown_panels {
        let (status, answered) = call(
            &router,
            "ListRoutingControls",
            &json!({ "ControlPanelArn": arn }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("Control panel not found: '{arn}'"),
            "case {case}: expected the service's own wording, got {answered}"
        );
        assert_eq!(
            answered.get("resourceId").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case}: the refusal names the ARN asked for, got {answered}"
        );
    }
}

