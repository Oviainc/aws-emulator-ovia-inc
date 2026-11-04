// SPDX-License-Identifier: CC0-1.0

//! `UpdateControlPanel`, end to end over HTTP.
//!
//! The rule under test: `UpdateControlPanel` writes the name the request carries onto the
//! stored control panel the ARN identifies, leaves every other member of that record
//! alone, and answers with the record a later read answers with; a name already held by
//! another panel of the same cluster is refused on the request's input (400), and an ARN
//! no control panel is stored under is refused with the modelled not-found error (404).
//!
//! The tables span what that rule generalises over: the NAME the request carries (a short
//! name, both published length bounds of `ControlPanelName` — len 1..64 pattern `^\S+$`,
//! via `smithy_query(query="operation",operation="UpdateControlPanel")` — a name holding
//! every other character class the pattern allows, the name the panel already has, and a
//! name another CLUSTER's panel holds, which is free because the name is scoped to one
//! cluster), WHICH panel is renamed (a caller-created one and the default panel a cluster
//! is created with), and whether the panel holds routing controls.

use serde_json::{json, Value};

use super::common::{call, create_cluster, create_control_panel, create_routing_control, enc,
    error_code, router};
use super::recovery_control_lifecycle::{
    ACCOUNT, ARN_PREFIX, PAST_DEPLOYMENT, STATUS_DEPLOYED, STATUS_PENDING,
};

/// Call `UpdateControlPanel` for `panel_arn` and `name`.
async fn update_control_panel(
    router: &smithy_server_runtime::MultiServiceRouter,
    panel_arn: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        "/controlpanel",
        Some(json!({ "ControlPanelArn": panel_arn, "ControlPanelName": name })),
    )
    .await
}

/// `DescribeControlPanel` for `panel_arn`, as the stored `ControlPanel` body.
async fn describe_control_panel(
    router: &smithy_server_runtime::MultiServiceRouter,
    panel_arn: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/controlpanel/{}", enc(panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {panel_arn}: expected DescribeControlPanel 200, got {status} {body:?}"
    );
    body["ControlPanel"].clone()
}

/// A cluster and one control panel inside it, as `(cluster ARN, the created panel)`.
async fn cluster_with_panel(
    router: &smithy_server_runtime::MultiServiceRouter,
    label: &str,
) -> (String, Value) {
    let cluster = create_cluster(router, json!({ "ClusterName": format!("cluster-{label}") })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let panel = create_control_panel(
        router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": format!("panel-{label}") }),
    )
    .await;
    (cluster_arn, panel)
}

/// A rename writes the requested name onto the stored panel, leaves everything else about
/// it alone, and a later read answers the same body.
#[tokio::test]
async fn update_writes_the_name_and_describe_answers_it() {
    let router = router();

    // A panel of ANOTHER cluster holds this name throughout, so the row that renames onto
    // it proves the name is scoped to one cluster rather than to the account.
    let (_, foreign) = cluster_with_panel(&router, "foreign").await;
    let foreign_name = foreign["Name"].as_str().unwrap_or_default().to_string();

    // Rows: the name the request carries. Each satisfies `ControlPanelName`'s modelled
    // constraints (len 1..64, pattern ^\S+$); the two length rows are the published
    // bounds, and `keeps-its-name` is the name the panel already holds.
    let cases: Vec<(&str, String)> = vec![
        ("a short name", "renamed".to_string()),
        ("a one-character name (the length minimum)", "x".to_string()),
        ("a 64-character name (the length maximum)", "n".repeat(64)),
        (
            "every other character class the pattern allows",
            "Panel_9-ARC:prod/eu".to_string(),
        ),
        ("the name the panel already holds", String::new()),
        ("a name another cluster's panel holds", foreign_name.clone()),
    ];

    for (row, (case, requested)) in cases.into_iter().enumerate() {
        let (cluster_arn, created) = cluster_with_panel(&router, &format!("row{row}")).await;
        let panel_arn = created["ControlPanelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The empty row stands for "the name it already holds", which only the created
        // panel knows.
        let requested = if requested.is_empty() {
            created["Name"].as_str().unwrap_or_default().to_string()
        } else {
            requested.clone()
        };

        let (status, answer) = update_control_panel(&router, &panel_arn, &requested).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateControlPanel 200, got {status} {answer:?}"
        );
        let updated = answer["ControlPanel"].clone();

        assert_eq!(
            updated["Name"], json!(requested),
            "case {case}: expected the requested name {requested:?}, got {:?}",
            updated["Name"]
        );
        for member in [
            "ControlPanelArn",
            "ClusterArn",
            "DefaultControlPanel",
            "Owner",
            "RoutingControlCount",
        ] {
            assert_eq!(
                updated[member], created[member],
                "case {case}: UpdateControlPanel must not change {member}, got {:?} after {:?}",
                updated[member], created[member]
            );
        }
        assert_eq!(
            updated["ClusterArn"], json!(cluster_arn),
            "case {case}: expected the panel to stay in its cluster, got {:?}",
            updated["ClusterArn"]
        );
        assert_eq!(
            updated["Owner"], json!(ACCOUNT),
            "case {case}: expected owner {ACCOUNT}, got {:?}",
            updated["Owner"]
        );

        // The response came from the stored record, so the read answers the same body.
        let read = describe_control_panel(&router, &panel_arn).await;
        for member in [
            "ControlPanelArn",
            "ClusterArn",
            "DefaultControlPanel",
            "Owner",
            "Name",
            "RoutingControlCount",
        ] {
            assert_eq!(
                read[member], updated[member],
                "case {case}: DescribeControlPanel must answer the written {member}, got {:?} \
                 after {:?}",
                read[member], updated[member]
            );
        }
    }

    // The panel of the other cluster never moved: a rename touches one panel.
    let foreign_arn = foreign["ControlPanelArn"].as_str().unwrap_or_default();
    let foreign_read = describe_control_panel(&router, foreign_arn).await;
    assert_eq!(
        foreign_read["Name"], json!(foreign_name),
        "case the other cluster's panel: expected its name untouched, got {:?}",
        foreign_read["Name"]
    );
}

/// The default control panel a cluster is created with is renamed the same way, and the
/// routing controls a panel holds are untouched by a rename: its `RoutingControlCount` and
/// the controls themselves survive it.
#[tokio::test]
async fn the_default_panel_and_a_panel_with_controls_are_renamed_too() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "default-panel" })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The panel the cluster was created with: found by listing the cluster's panels.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/controlpanels?ClusterArn={}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list the cluster's panels: expected 200, got {status} {listed:?}"
    );
    let default_panel = listed["ControlPanels"]
        .as_array()
        .and_then(|panels| {
            panels
                .iter()
                .find(|panel| panel["DefaultControlPanel"] == json!(true))
        })
        .cloned()
        .unwrap_or(Value::Null);
    let default_arn = default_panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !default_arn.is_empty(),
        "case the default panel: expected the cluster to hold one, got {listed:?}"
    );

    // Two rows: the default panel, and a caller-created panel holding routing controls.
    let with_controls = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "holds-controls" }),
    )
    .await;
    let with_controls_arn = with_controls["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for n in 0..3 {
        create_routing_control(
            &router,
            json!({
                "ClusterArn": cluster_arn,
                "ControlPanelArn": with_controls_arn,
                "RoutingControlName": format!("control-{n}"),
            }),
        )
        .await;
    }

    let rows: Vec<(&str, String, &str, i64)> = vec![
        ("the default panel", default_arn, "renamed-default", 0),
        (
            "a panel holding three routing controls",
            with_controls_arn.clone(),
            "renamed-holder",
            3,
        ),
    ];

    for (case, panel_arn, name, expected_count) in rows {
        let (status, answer) = update_control_panel(&router, &panel_arn, name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateControlPanel 200, got {status} {answer:?}"
        );
        assert_eq!(
            answer["ControlPanel"]["Name"], json!(name),
            "case {case}: expected the requested name {name}, got {:?}",
            answer["ControlPanel"]["Name"]
        );
        assert_eq!(
            answer["ControlPanel"]["RoutingControlCount"], json!(expected_count),
            "case {case}: expected RoutingControlCount {expected_count}, got {:?}",
            answer["ControlPanel"]["RoutingControlCount"]
        );

        let read = describe_control_panel(&router, &panel_arn).await;
        assert_eq!(
            read["Name"], json!(name),
            "case {case}: DescribeControlPanel must answer the written name, got {:?}",
            read["Name"]
        );
        assert_eq!(
            read["RoutingControlCount"], json!(expected_count),
            "case {case}: a rename must not change the panel's controls, got {:?}",
            read["RoutingControlCount"]
        );
    }

    // The routing controls of the renamed panel are all still there, under the panel they
    // were created in.
    let (status, controls) = call(
        &router,
        "GET",
        &format!("/controlpanel/{}/routingcontrols", enc(&with_controls_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list the renamed panel's controls: expected 200, got {status} {controls:?}"
    );
    let names: Vec<&str> = controls["RoutingControls"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|control| control["Name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["control-0", "control-1", "control-2"],
        "case list the renamed panel's controls: expected every control kept, got {names:?}"
    );
}

/// A rename that changes the name re-deploys the panel: it reads `PENDING` straight after
/// the call and reaches `DEPLOYED` on its own. A request naming the name the panel already
/// holds changes nothing, so it does not put a deployed panel back into `PENDING`.
#[tokio::test]
async fn a_changing_rename_redeploys_the_panel_and_a_no_op_does_not() {
    let router = router();

    for (case, changes) in [("a changed name", true), ("the same name", false)] {
        let (_, created) = cluster_with_panel(&router, &case.replace(' ', "-")).await;
        let panel_arn = created["ControlPanelArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let current = created["Name"].as_str().unwrap_or_default().to_string();

        // Wait the panel's first deployment out, so the status the rename leaves is the
        // only thing the next read can be reporting.
        std::thread::sleep(PAST_DEPLOYMENT);
        let before = describe_control_panel(&router, &panel_arn).await;
        assert_eq!(
            before["Status"], json!(STATUS_DEPLOYED),
            "case {case}: the created panel must be {STATUS_DEPLOYED} before the rename, got {:?}",
            before["Status"]
        );

        let requested = if changes {
            format!("{current}-changed")
        } else {
            current.clone()
        };
        let (status, answer) = update_control_panel(&router, &panel_arn, &requested).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateControlPanel 200, got {status} {answer:?}"
        );
        let expected_status = if changes {
            STATUS_PENDING
        } else {
            STATUS_DEPLOYED
        };
        assert_eq!(
            answer["ControlPanel"]["Status"], json!(expected_status),
            "case {case}: expected {expected_status} right after the rename, got {:?}",
            answer["ControlPanel"]["Status"]
        );

        // Nothing writes the transition: waiting the span out is what makes it due.
        std::thread::sleep(PAST_DEPLOYMENT);
        let after = describe_control_panel(&router, &panel_arn).await;
        assert_eq!(
            after["Status"], json!(STATUS_DEPLOYED),
            "case {case}: expected {STATUS_DEPLOYED} once the deployment is due, got {:?}",
            after["Status"]
        );
        assert_eq!(
            after["Name"], json!(requested),
            "case {case}: the written name must survive the deployment, got {:?}",
            after["Name"]
        );
    }
}

/// A name another panel of the SAME cluster already holds is refused on the request's
/// input (400) with the sentence the create states, and the panel keeps the name it had.
#[tokio::test]
async fn renaming_onto_a_name_taken_in_the_same_cluster_is_refused() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "collision" })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let first = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "taken" }),
    )
    .await;
    let second = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "free" }),
    )
    .await;
    let second_arn = second["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: the name of a sibling the caller created, and the name of the default panel
    // the cluster was created with — both are names taken inside this cluster.
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/controlpanels?ClusterArn={}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list the cluster's panels: expected 200, got {status} {listed:?}"
    );
    let default_name = listed["ControlPanels"]
        .as_array()
        .and_then(|panels| {
            panels
                .iter()
                .find(|panel| panel["DefaultControlPanel"] == json!(true))
        })
        .and_then(|panel| panel["Name"].as_str())
        .unwrap_or_default()
        .to_string();

    for (case, taken) in [
        ("a sibling panel's name", "taken".to_string()),
        ("the default panel's name", default_name),
    ] {
        let (status, answer) = update_control_panel(&router, &second_arn, &taken).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
        let actual = answer["message"].as_str().unwrap_or_default();
        let expected = format!("Control Panel {taken} already exists in Cluster {cluster_arn}.");
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );

        // The refused rename wrote nothing: both panels keep their names.
        let read = describe_control_panel(&router, &second_arn).await;
        assert_eq!(
            read["Name"], second["Name"],
            "case {case}: a refused rename must not write, got {:?}",
            read["Name"]
        );
    }

    let first_read = describe_control_panel(
        &router,
        first["ControlPanelArn"].as_str().unwrap_or_default(),
    )
    .await;
    assert_eq!(
        first_read["Name"], first["Name"],
        "case the panel holding the name: it must be untouched, got {:?}",
        first_read["Name"]
    );

    // And a panel may be renamed onto the name it ALREADY holds: the check excludes the
    // panel the request addresses.
    let (status, answer) = update_control_panel(&router, &second_arn, "free").await;
    assert_eq!(
        status, 200,
        "case renaming a panel onto its own name: expected 200, got {status} {answer:?}"
    );
}

/// An ARN no control panel is stored under is `ResourceNotFoundException` (404), whatever
/// resource the ARN names.
#[tokio::test]
async fn renaming_an_unknown_panel_is_not_found() {
    let router = router();

    // A real panel exists, so a refusal is about the ARN the request names and not about
    // an empty store.
    let (cluster_arn, existing) = cluster_with_panel(&router, "present").await;
    let existing_arn = existing["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let missing_panel = format!("{ARN_PREFIX}controlpanel/00000000-0000-0000-0000-000000000000");
    let doomed = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "doomed" }),
    )
    .await;
    let doomed_arn = doomed["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/controlpanel/{}", enc(&doomed_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case prepare a deleted panel: expected DeleteControlPanel 200, got {status} {body:?}"
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "a control-panel ARN nothing is stored under",
            missing_panel.clone(),
        ),
        ("a cluster ARN", cluster_arn.clone()),
        (
            "a routing-control ARN",
            format!("{missing_panel}/routingcontrol/11111111-1111-1111-1111-111111111111"),
        ),
        ("a deleted panel's ARN", doomed_arn),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in cases {
        let (status, answer) = update_control_panel(&router, &arn, "renamed").await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
        let actual = answer["message"].as_str().unwrap_or_default();
        let expected = format!("No record corresponding to control panel Arn({arn})");
        assert_eq!(
            actual, expected,
            "case {case}: expected the fixed not-found sentence, got {actual:?}"
        );
    }

    // The panel that does exist still carries the name it was created with.
    let read = describe_control_panel(&router, &existing_arn).await;
    assert_eq!(
        read["Name"], existing["Name"],
        "case the existing panel: a refused rename must change nothing, got {:?} after {:?}",
        read["Name"], existing["Name"]
    );
}

/// A `ControlPanelName` outside the modelled constraints never reaches this operation: the
/// request-body validator refuses it with `BadRequestException` (400) — the class
/// `tests/all/rejection_error_classes.rs` states the rule for — and the panel keeps its
/// name. The rows are the two published length bounds, one past each, and the pattern.
#[tokio::test]
async fn a_name_outside_the_modelled_constraints_is_refused() {
    let router = router();

    let (_, created) = cluster_with_panel(&router, "constraints").await;
    let panel_arn = created["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let original = created["Name"].as_str().unwrap_or_default().to_string();

    // `ControlPanelName` is len 1..64, pattern ^\S+$
    // (`smithy_query(query="operation",operation="UpdateControlPanel")`): one below the
    // minimum, one above the maximum, and three ways to break the pattern.
    let refused: Vec<(&str, String)> = vec![
        ("empty (one below the length minimum)", String::new()),
        (
            "65 characters (one past the length maximum)",
            "a".repeat(65),
        ),
        ("an inner space", "has space".to_string()),
        ("a leading space", " leading".to_string()),
        ("a tab", "has\tTab".to_string()),
    ];

    for (case, name) in refused {
        let (status, answer) = update_control_panel(&router, &panel_arn, &name).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {answer:?}"
        );
        let read = describe_control_panel(&router, &panel_arn).await;
        assert_eq!(
            read["Name"], json!(original),
            "case {case}: a refused rename must not write, got {:?}",
            read["Name"]
        );
    }

    // Both published bounds are inside the constraint and must be served.
    for name in ["x", &"b".repeat(64)] {
        let (status, answer) = update_control_panel(&router, &panel_arn, name).await;
        assert_eq!(
            status, 200,
            "case name {name:?} (a published bound): expected 200, got {status} {answer:?}"
        );
        assert_eq!(
            answer["ControlPanel"]["Name"], json!(name),
            "case name {name:?} (a published bound): expected it stored, got {:?}",
            answer["ControlPanel"]["Name"]
        );
    }
}
