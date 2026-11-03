// SPDX-License-Identifier: CC0-1.0

//! `ModifyDBClusterParameterGroup`'s value rule, over EVERY cluster parameter catalog this
//! one `rds` front door publishes.
//!
//! The rule: a `ParameterValue` must be one the parameter's published `AllowedValues`
//! admits — "the valid range of values for the parameter" — in the group's own
//! `DBParameterGroupFamily` catalog, the same one `DescribeDBClusterParameters` and
//! `DescribeEngineDefaultClusterParameters` report; a value outside it is refused with HTTP
//! 400 `InvalidParameterValue` and nothing in the request is applied. An Amazon RDS, Amazon
//! DocumentDB or Amazon Neptune SDK client all sign `rds`, so all three control planes'
//! families are served here and all three are rows below.
//!
//! The table is each family's published catalog ITSELF, so every parameter of every cluster
//! engine this front door serves is exercised — each enumerated value, each published range
//! bound, one step past each bound, a value inside the range where the bounds leave room,
//! and a value of the wrong type — rather than the one parameter and the one value any
//! recorded request used.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    published_cluster_parameters, ClusterParameterSpec, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_parameter_groups::{admitted_and_refused, create_group, modify};
use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text};

/// Two cluster parameter group families per cluster engine vocabulary this front door
/// serves: the families are `DBParameterGroupFamily` values the engine-version catalogue
/// publishes (`DescribeDBEngineVersions`), and the rule is the same for every one of them.
const FAMILIES: [(&str, &str); 8] = [
    ("Aurora MySQL", "aurora-mysql8.0"),
    ("RDS for MySQL in a Multi-AZ DB cluster", "mysql8.0"),
    ("Aurora PostgreSQL", "aurora-postgresql16"),
    ("RDS for PostgreSQL in a Multi-AZ DB cluster", "postgres16"),
    ("Amazon DocumentDB", "docdb5.0"),
    ("Amazon DocumentDB, an earlier family", "docdb3.6"),
    ("Amazon Neptune", "neptune1.4"),
    ("Amazon Neptune, an earlier family", "neptune1.2"),
];

/// A value strictly INSIDE a published numeric range, where its bounds leave room for one:
/// the midpoint of the two, read from the parameter's own published range rather than named
/// here. A range of adjacent bounds (`0-1`) and an enumeration have none.
fn inside_the_range(spec: &ClusterParameterSpec) -> Option<String> {
    if spec.allowed_values.contains(',') {
        return None;
    }
    let (low, high) = spec.range()?;
    let low: i64 = low.trim().parse().ok()?;
    let high: i64 = high.trim().parse().ok()?;
    (high - low >= 2).then(|| (low + (high - low) / 2).to_string())
}

/// The value the group reports as its OWN (`Source` `user`) for `name`, or `None` where it
/// carries no override for it — the half of `DescribeDBClusterParameters` a modify writes.
async fn user_value(router: &MultiServiceRouter, group: &str, name: &str) -> Option<String> {
    let reported = expect_ok(
        name,
        call(
            router,
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", group),
                ("Source", PARAMETER_SOURCE_USER),
            ],
        )
        .await,
    );
    subtrees(&reported, "Parameter")
        .into_iter()
        .find(|parameter| text(parameter, "ParameterName") == name)
        .map(|parameter| text(&parameter, "ParameterValue"))
}

/// The refusal a value outside a parameter's published domain is answered with, in the two
/// wordings the two FORMS of that domain take: a numeric range names the range, an
/// enumeration names the values. Both are built from the request and that parameter's own
/// catalog entry.
fn expected_message(spec: &ClusterParameterSpec, value: &str) -> String {
    if spec.publishes_a_range() {
        format!(
            "Value: {value} is outside of range: {} for parameter: {}",
            spec.allowed_values, spec.name
        )
    } else {
        format!(
            "Invalid parameter value: {value} for: {} allowed values are: {}",
            spec.name, spec.allowed_values
        )
    }
}

/// Every parameter of every cluster family this front door publishes a catalog for admits
/// exactly what its `AllowedValues` says and refuses everything else with HTTP 400
/// `InvalidParameterValue`, and the refusal changes nothing: the group still reports the
/// value it carried before.
#[tokio::test]
async fn a_cluster_parameter_value_outside_its_published_range_is_refused() {
    let router = emulator();

    for (index, (case, family)) in FAMILIES.into_iter().enumerate() {
        let Some(catalog) = published_cluster_parameters(family) else {
            panic!("case {case:?}: expected {family} to publish a cluster parameter catalog");
        };
        assert!(
            !catalog.is_empty(),
            "case {case:?}: expected {family} to publish at least one cluster parameter"
        );
        let group = create_group(&router, case, &format!("range-cpg-{index}"), family).await;

        for spec in catalog {
            let (mut admitted, refused) = admitted_and_refused(spec);
            admitted.extend(inside_the_range(spec));
            // A value the parameter admits is stored and read back from state.
            for value in &admitted {
                let case = format!("{family} {} = {value:?} (admitted)", spec.name);
                expect_ok(&case, modify(&router, &group, spec.name, value, None).await);
                let stored = user_value(&router, &group, spec.name).await;
                assert_eq!(
                    stored.as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: expected the group to report the stored value, got \
                     {stored:?}"
                );
            }
            // ...and one it does not is refused, in the wording its published domain's form
            // takes, leaving the value the group already carried in place.
            let carried = admitted.last().cloned();
            for value in &refused {
                let case = format!("{family} {} = {value:?} (refused)", spec.name);
                let answered = expect_error(
                    &case,
                    modify(&router, &group, spec.name, value, None).await,
                    400,
                    "InvalidParameterValue",
                );
                let message = text(&answered, "Message");
                assert_eq!(
                    message,
                    expected_message(spec, value),
                    "case {case:?}: expected the published refusal wording, got {message:?}"
                );
                let stored = user_value(&router, &group, spec.name).await;
                assert_eq!(
                    stored, carried,
                    "case {case:?}: a refused value must leave the group as it was, got \
                     {stored:?}"
                );
            }
        }
    }
}

/// The refusal is ATOMIC over the whole request: a batch carrying one value outside its
/// parameter's published range applies NONE of its parameters, so a group that carried no
/// override still carries none afterwards and the same batch with that value brought inside
/// the range applies all of them.
#[tokio::test]
async fn one_out_of_range_value_leaves_the_whole_batch_unapplied() {
    let router = emulator();
    // (case, family, the parameter whose value the row puts out of range, a parameter of the
    // same family whose value is admitted and the value it takes)
    let rows: [(&str, &str, &str, &str, &str); 3] = [
        (
            "an Amazon Neptune family",
            "neptune1.4",
            "neptune_streams_expiry_days",
            "neptune_streams",
            "1",
        ),
        (
            "an Aurora MySQL family",
            "aurora-mysql8.0",
            "lower_case_table_names",
            "binlog_format",
            "ROW",
        ),
        (
            "an Amazon DocumentDB family",
            "docdb5.0",
            "profiler_threshold_ms",
            "profiler",
            "enabled",
        ),
    ];

    for (index, (case, family, ranged, admitted_name, admitted_value)) in
        rows.into_iter().enumerate()
    {
        let spec = published_cluster_parameters(family)
            .and_then(|catalog| catalog.iter().find(|spec| spec.name == ranged))
            .unwrap_or_else(|| panic!("case {case:?}: expected {family} to publish {ranged}"));
        let (low, high) = spec
            .range()
            .unwrap_or_else(|| panic!("case {case:?}: expected {ranged} to publish a range"));
        let high: i64 = high
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected an integer upper bound"));
        let past_the_bound = (high + 1).to_string();
        let group = create_group(&router, case, &format!("atomic-cpg-{index}"), family).await;

        let batch = |value: &str| {
            vec![
                ("DBClusterParameterGroupName".to_string(), group.clone()),
                (
                    "Parameters.Parameter.1.ParameterName".to_string(),
                    admitted_name.to_string(),
                ),
                (
                    "Parameters.Parameter.1.ParameterValue".to_string(),
                    admitted_value.to_string(),
                ),
                (
                    "Parameters.Parameter.1.ApplyMethod".to_string(),
                    "pending-reboot".to_string(),
                ),
                (
                    "Parameters.Parameter.2.ParameterName".to_string(),
                    ranged.to_string(),
                ),
                (
                    "Parameters.Parameter.2.ParameterValue".to_string(),
                    value.to_string(),
                ),
                (
                    "Parameters.Parameter.2.ApplyMethod".to_string(),
                    "pending-reboot".to_string(),
                ),
            ]
        };

        expect_error(
            case,
            call(
                &router,
                "ModifyDBClusterParameterGroup",
                &batch(&past_the_bound),
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        for name in [admitted_name, ranged] {
            let stored = user_value(&router, &group, name).await;
            assert_eq!(
                stored, None,
                "case {case:?}: the refused batch must have applied nothing, yet {name} \
                 reports {stored:?}"
            );
        }

        // The same batch, with that one value inside the published range, applies both.
        let inside = inside_the_range(spec).unwrap_or_else(|| low.trim().to_string());
        expect_ok(
            case,
            call(&router, "ModifyDBClusterParameterGroup", &batch(&inside)).await,
        );
        for (name, expected) in [(admitted_name, admitted_value), (ranged, inside.as_str())] {
            let stored = user_value(&router, &group, name).await;
            assert_eq!(
                stored.as_deref(),
                Some(expected),
                "case {case:?}: expected {name} to report {expected:?}, got {stored:?}"
            );
        }
    }
}
