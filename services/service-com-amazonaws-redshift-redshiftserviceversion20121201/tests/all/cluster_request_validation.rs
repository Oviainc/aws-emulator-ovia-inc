// SPDX-License-Identifier: CC0-1.0

//! What `CreateCluster` REFUSES, end to end over the awsQuery wire, and the
//! reference check `DeleteClusterParameterGroup` makes before it removes a group.
//!
//! The rule under test: **every value a cluster write carries is checked against
//! the constraint the service publishes for that member BEFORE anything is
//! provisioned, and a value outside it is refused — with `InvalidParameterValue`
//! for a value the member cannot take, and with the resource's own modelled
//! NotFound fault for a name that resolves to nothing — so no cluster whose
//! configuration the service would not accept is ever stored.** The mirror of it
//! on the way out: **a resource a live cluster is a member of is not the
//! account's to delete**, which is what keeps a running cluster from losing the
//! parameter group it is configured from.
//!
//! Every row drives a real HTTP request through the same `MultiServiceRouter` the
//! production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The rows span the classes the rule generalises over rather than one recorded
//! request: the node-count/cluster-type pairing in both directions and at each
//! bound, the maintenance window's documented FORM and its documented minimum
//! LENGTH (at the bound, one minute short of it, wrapping the week, and
//! malformed), a parameter group name that resolves to nothing, and a maintenance
//! track outside the published catalog — beside the values at the edge of each
//! constraint that must be ACCEPTED, because a rule that refuses everything is
//! not the rule.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The parameter group the accepted rows and the in-use check name.
const GROUP: &str = "validation-group";

/// One row of the refusal table: the case, the members it breaks on the valid
/// request, and the `(status, code, message)` the service must answer with.
struct Refused {
    case: &'static str,
    overrides: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
    message: String,
}

/// One row of the acceptance table: the case and the members it carries beyond
/// the valid request — each at the EDGE of a constraint the refusals above are
/// outside of.
struct Accepted {
    case: &'static str,
    overrides: Vec<(&'static str, &'static str)>,
}

/// `CreateCluster` with the three required members, `identifier`, and `overrides`
/// applied on top.
async fn create(
    emulator: &Emulator,
    identifier: &str,
    overrides: &[(&'static str, &'static str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", identifier),
        ("NodeType", "ra3.xlplus"),
        ("MasterUsername", "admin"),
        ("ClusterType", "single-node"),
    ];
    // An override REPLACES the baseline's own pair for that member, so a row that
    // changes the cluster type does not send two of them.
    for (member, value) in overrides {
        match params.iter_mut().find(|(name, _)| name == member) {
            Some(existing) => existing.1 = value,
            None => params.push((member, value)),
        }
    }
    emulator.call("CreateCluster", &params).await
}

/// Seed the parameter group the rows name.
async fn seed_group(emulator: &Emulator) {
    let created = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", GROUP),
                (
                    "ParameterGroupFamily",
                    state::DEFAULT_PARAMETER_GROUP_FAMILY,
                ),
                ("Description", "the group the validation rows name"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case seed {GROUP}: expected 200, got {} with body {}",
        created.status, created.body
    );
}

#[tokio::test]
async fn a_create_is_refused_before_anything_is_provisioned() {
    let emulator = Emulator::new();
    seed_group(&emulator).await;

    let rows: Vec<Refused> = vec![
        Refused {
            case: "two nodes asked of the single-node shape",
            overrides: vec![("NumberOfNodes", "2")],
            status: 400,
            code: "InvalidParameterValue",
            message: state::SINGLE_NODE_COUNT_REFUSAL.to_string(),
        },
        Refused {
            case: "many nodes asked of the single-node shape",
            overrides: vec![("NumberOfNodes", "16")],
            status: 400,
            code: "InvalidParameterValue",
            message: state::SINGLE_NODE_COUNT_REFUSAL.to_string(),
        },
        Refused {
            case: "one node asked of the multi-node shape",
            overrides: vec![("ClusterType", "multi-node"), ("NumberOfNodes", "1")],
            status: 400,
            code: "InvalidParameterValue",
            message: state::MULTI_NODE_COUNT_REFUSAL.to_string(),
        },
        Refused {
            case: "no node at all asked of the multi-node shape",
            overrides: vec![("ClusterType", "multi-node"), ("NumberOfNodes", "0")],
            status: 400,
            code: "InvalidParameterValue",
            message: state::MULTI_NODE_COUNT_REFUSAL.to_string(),
        },
        Refused {
            case: "the multi-node shape with no count named",
            overrides: vec![("ClusterType", "multi-node")],
            status: 400,
            code: "InvalidParameterValue",
            message: state::MULTI_NODE_COUNT_REFUSAL.to_string(),
        },
        Refused {
            case: "a cluster type the service does not publish",
            overrides: vec![("ClusterType", "mega-node")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Invalid value for ClusterType: 'mega-node'. Valid Values: {}",
                state::ORDERABLE_CLUSTER_TYPES.join(" | ")
            ),
        },
        Refused {
            case: "a parameter group name that resolves to nothing",
            overrides: vec![("ClusterParameterGroupName", "no-such-group")],
            status: 404,
            code: "ClusterParameterGroupNotFound",
            message: "ClusterParameterGroup not found: no-such-group".to_string(),
        },
        Refused {
            case: "a maintenance window shorter than the documented minimum",
            overrides: vec![("PreferredMaintenanceWindow", "wed:04:00-wed:04:10")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Maintenance window must be at least {}minutes.",
                state::MIN_MAINTENANCE_WINDOW_MINUTES
            ),
        },
        Refused {
            case: "a maintenance window one minute short of the documented minimum",
            overrides: vec![("PreferredMaintenanceWindow", "wed:04:00-wed:04:29")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Maintenance window must be at least {}minutes.",
                state::MIN_MAINTENANCE_WINDOW_MINUTES
            ),
        },
        Refused {
            case: "a maintenance window that spans nothing",
            overrides: vec![("PreferredMaintenanceWindow", "wed:04:00-wed:04:00")],
            status: 400,
            code: "InvalidParameterValue",
            message: format!(
                "Maintenance window must be at least {}minutes.",
                state::MIN_MAINTENANCE_WINDOW_MINUTES
            ),
        },
        Refused {
            case: "a maintenance window that is not a range at all",
            overrides: vec![("PreferredMaintenanceWindow", "wed:04:00")],
            status: 400,
            code: "InvalidParameterValue",
            message: malformed_window_message("wed:04:00"),
        },
        Refused {
            case: "a maintenance window naming a day the service does not publish",
            overrides: vec![("PreferredMaintenanceWindow", "xyz:04:00-xyz:05:00")],
            status: 400,
            code: "InvalidParameterValue",
            message: malformed_window_message("xyz:04:00-xyz:05:00"),
        },
        Refused {
            case: "a maintenance window whose hour is outside the clock",
            overrides: vec![("PreferredMaintenanceWindow", "wed:24:00-wed:25:00")],
            status: 400,
            code: "InvalidParameterValue",
            message: malformed_window_message("wed:24:00-wed:25:00"),
        },
        Refused {
            case: "a maintenance window whose clock fields are signed",
            overrides: vec![("PreferredMaintenanceWindow", "wed:+4:00-wed:+5:00")],
            status: 400,
            code: "InvalidParameterValue",
            message: malformed_window_message("wed:+4:00-wed:+5:00"),
        },
        Refused {
            case: "a maintenance window whose clock fields are not two digits",
            overrides: vec![("PreferredMaintenanceWindow", "wed:4:00-wed:5:00")],
            status: 400,
            code: "InvalidParameterValue",
            message: malformed_window_message("wed:4:00-wed:5:00"),
        },
        Refused {
            case: "a maintenance track outside the published catalog",
            overrides: vec![("MaintenanceTrackName", "nosuch")],
            status: 400,
            code: "InvalidClusterTrack",
            message: "Invalid cluster track: nosuch.".to_string(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let identifier = format!("refused-{index:02}");
        let answer = create(&emulator, &identifier, &row.overrides).await;
        assert_eq!(
            (answer.status, answer.error_code().as_deref()),
            (row.status, Some(row.code)),
            "case {}: expected {} {}, got {} / {:?} with body {}",
            row.case,
            row.status,
            row.code,
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(row.message.as_str()),
            "case {}: expected the message to name the member and its constraint, got {:?}",
            row.case,
            answer.text("Message")
        );
        // A refused create provisions NOTHING: the identifier it named is still
        // free, which is what makes the check a precondition rather than a
        // cleanup.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        assert_eq!(
            (described.status, described.error_code().as_deref()),
            (404, Some("ClusterNotFound")),
            "case {}: expected the refused create to leave no cluster, got {} / {:?}",
            row.case,
            described.status,
            described.error_code()
        );
    }
}

/// The refusal a window outside the documented `ddd:hh24:mi-ddd:hh24:mi` form
/// carries, for `window`.
fn malformed_window_message(window: &str) -> String {
    format!(
        "Invalid value for PreferredMaintenanceWindow: '{window}'. The window must be in the format ddd:hh24:mi-ddd:hh24:mi, with each day one of {}.",
        state::MAINTENANCE_WINDOW_DAYS.join(" | ")
    )
}

#[tokio::test]
async fn a_value_at_the_edge_of_its_constraint_is_accepted() {
    let emulator = Emulator::new();
    seed_group(&emulator).await;

    let rows: Vec<Accepted> = vec![
        Accepted {
            case: "the single-node shape with its one node named",
            overrides: vec![("NumberOfNodes", "1")],
        },
        Accepted {
            case: "the multi-node shape at its lowest node count",
            overrides: vec![
                ("ClusterType", "multi-node"),
                (
                    "NumberOfNodes",
                    // The published floor of the multi-node shape.
                    "2",
                ),
            ],
        },
        Accepted {
            case: "a maintenance window exactly at the documented minimum",
            overrides: vec![("PreferredMaintenanceWindow", "wed:04:00-wed:04:30")],
        },
        Accepted {
            case: "a maintenance window that wraps the week",
            overrides: vec![("PreferredMaintenanceWindow", "sun:23:45-mon:00:30")],
        },
        Accepted {
            case: "a maintenance window spanning days",
            overrides: vec![("PreferredMaintenanceWindow", "mon:00:00-thu:12:00")],
        },
        Accepted {
            case: "the parameter group the account holds",
            overrides: vec![("ClusterParameterGroupName", GROUP)],
        },
        Accepted {
            case: "a maintenance track the catalog publishes",
            overrides: vec![("MaintenanceTrackName", state::TRAILING_MAINTENANCE_TRACK)],
        },
    ];

    // The floor the multi-node row names is the one the service publishes.
    assert_eq!(
        state::MIN_MULTI_NODE_NODES, 2,
        "the multi-node row names the published floor"
    );

    for (index, row) in rows.iter().enumerate() {
        let identifier = format!("accepted-{index:02}");
        let answer = create(&emulator, &identifier, &row.overrides).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected the create to be accepted, got {} / {:?} with body {}",
            row.case,
            answer.status,
            answer.error_code(),
            answer.body
        );
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        assert_eq!(
            described.text("ClusterIdentifier").as_deref(),
            Some(identifier.as_str()),
            "case {}: expected the accepted create to be stored, got body {}",
            row.case,
            described.body
        );
    }
}

#[tokio::test]
async fn a_parameter_group_a_live_cluster_is_a_member_of_cannot_be_deleted() {
    let emulator = Emulator::new();
    seed_group(&emulator).await;

    let created = create(
        &emulator,
        "group-member",
        &[("ClusterParameterGroupName", GROUP)],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create the member cluster: expected 200, got {} with body {}",
        created.status, created.body
    );

    let refused = emulator
        .call("DeleteClusterParameterGroup", &[("ParameterGroupName", GROUP)])
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_deref()),
        (400, Some("InvalidClusterParameterGroupState")),
        "case delete a group in use: expected 400 InvalidClusterParameterGroupState, got {} / {:?} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        refused.text("Message").as_deref(),
        Some(
            format!(
                "One or more Clusters are still members of this parameter group {GROUP}, so the group cannot be deleted"
            )
            .as_str()
        ),
        "case delete a group in use: expected the message to name the group, got {:?}",
        refused.text("Message")
    );
    // The refusal persisted nothing: the group is still there for the cluster
    // that is configured from it.
    let still_there = emulator
        .call(
            "DescribeClusterParameterGroups",
            &[("ParameterGroupName", GROUP)],
        )
        .await;
    assert_eq!(
        still_there.text("ParameterGroupName").as_deref(),
        Some(GROUP),
        "case delete a group in use: expected the group to survive the refusal, got body {}",
        still_there.body
    );

    // Once no cluster is a member of it, the same delete succeeds — the group was
    // refused for being IN USE, not for being undeletable.
    let deleted_cluster = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "group-member"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted_cluster.status, 200,
        "case delete the member cluster: expected 200, got {} with body {}",
        deleted_cluster.status, deleted_cluster.body
    );
    let deleted_group = emulator
        .call("DeleteClusterParameterGroup", &[("ParameterGroupName", GROUP)])
        .await;
    assert_eq!(
        deleted_group.status, 200,
        "case delete the group once free: expected 200, got {} / {:?} with body {}",
        deleted_group.status,
        deleted_group.error_code(),
        deleted_group.body
    );
}
