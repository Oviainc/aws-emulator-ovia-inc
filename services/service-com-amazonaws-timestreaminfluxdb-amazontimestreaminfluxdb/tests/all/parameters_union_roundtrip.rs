// SPDX-License-Identifier: CC0-1.0

//! The `Parameters` union of a DB parameter group, end to end over the wire.
//!
//! The suite drives real `awsJson1_0` requests through the same [`MultiServiceRouter`]
//! the production router dispatches with, reusing the harness in
//! [`crate::db_instance_lifecycle`] (`router`, `call`, `ok`) so there is one wire harness
//! for this crate.
//!
//! The rule under test: a DB parameter group configures exactly ONE engine parameter
//! family, so every shape that models `parameters` reports that single-member union — the
//! family the create request selected, echoed member for member, and the default family
//! (InfluxDB v2, with no tunable overridden) when the request selected none. The member is
//! therefore never absent from a group's create or read response, and a read reports the
//! union the group was stored with.
//!
//! The rows vary which family is selected, how many tunables are set inside it (none, one,
//! many), the types of those tunables (boolean, enum, integer, long, float, a nested
//! `Duration` structure, a nested `PercentOrAbsoluteLong` union) and whether the request
//! names a family at all.

use serde_json::{json, Value};

use crate::db_instance_lifecycle::{call, ok, router};

/// The members of the `Parameters` union, from `smithy_query(query="shape",
/// shape="Parameters")`: one engine parameter family each, and exactly one of them is set
/// on any value of the union.
const PARAMETER_FAMILIES: &[&str] = &["InfluxDBv2", "InfluxDBv3Core", "InfluxDBv3Enterprise"];

/// The family a group configures when its create request selects none: the default
/// engine's family with no tunable overridden. Every member of `InfluxDBv2Parameters` is
/// optional (`smithy_query(query="shape", shape="InfluxDBv2Parameters")`), so the
/// variant's value object carries none of them.
fn default_family() -> Value {
    json!({ "InfluxDBv2": {} })
}

/// One create request and the `parameters` every response about the group must report.
struct Case {
    name: &'static str,
    /// The `parameters` member of the request, or `None` when the request omits it.
    requested: Option<Value>,
    expected: Value,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no parameters member at all: the default family",
            requested: None,
            expected: default_family(),
        },
        Case {
            name: "the InfluxDBv2 family with no tunable set",
            requested: Some(json!({ "InfluxDBv2": {} })),
            expected: json!({ "InfluxDBv2": {} }),
        },
        Case {
            name: "the InfluxDBv2 family with one boolean tunable",
            requested: Some(json!({ "InfluxDBv2": { "uiDisabled": true } })),
            expected: json!({ "InfluxDBv2": { "uiDisabled": true } }),
        },
        Case {
            // `logLevel` is the modelled `LogLevel` enum, `queryConcurrency` an integer in
            // range 0..256, `influxqlMaxSelectPoint` a long, and `httpIdleTimeout` /
            // `storageRetentionCheckInterval` the nested `Duration` structure whose
            // `durationType` is the modelled `DurationType` enum.
            name: "the InfluxDBv2 family with an enum, booleans, numbers and durations",
            requested: Some(json!({
                "InfluxDBv2": {
                    "fluxLogEnabled": true,
                    "logLevel": "error",
                    "noTasks": false,
                    "queryConcurrency": 256,
                    "influxqlMaxSelectPoint": 1000000000000i64,
                    "httpIdleTimeout": { "durationType": "minutes", "value": 3 },
                    "storageRetentionCheckInterval": { "durationType": "hours", "value": 1 }
                }
            })),
            expected: json!({
                "InfluxDBv2": {
                    "fluxLogEnabled": true,
                    "logLevel": "error",
                    "noTasks": false,
                    "queryConcurrency": 256,
                    "influxqlMaxSelectPoint": 1000000000000i64,
                    "httpIdleTimeout": { "durationType": "minutes", "value": 3 },
                    "storageRetentionCheckInterval": { "durationType": "hours", "value": 1 }
                }
            }),
        },
        Case {
            name: "the InfluxDBv3Core family with no tunable set",
            requested: Some(json!({ "InfluxDBv3Core": {} })),
            expected: json!({ "InfluxDBv3Core": {} }),
        },
        Case {
            // `forceSnapshotMemThreshold` targets the `PercentOrAbsoluteLong` union: a
            // union nested inside the selected family's value object.
            name: "the InfluxDBv3Core family with the percent member of the nested union",
            requested: Some(json!({
                "InfluxDBv3Core": {
                    "queryFileLimit": 432,
                    "logFormat": "full",
                    "forceSnapshotMemThreshold": { "percent": "70%" },
                    "parquetMemCachePrunePercentage": 0.5,
                    "gen1Duration": { "durationType": "minutes", "value": 5 }
                }
            })),
            expected: json!({
                "InfluxDBv3Core": {
                    "queryFileLimit": 432,
                    "logFormat": "full",
                    "forceSnapshotMemThreshold": { "percent": "70%" },
                    "parquetMemCachePrunePercentage": 0.5,
                    "gen1Duration": { "durationType": "minutes", "value": 5 }
                }
            }),
        },
        Case {
            name: "the InfluxDBv3Core family with the absolute member of the nested union",
            requested: Some(json!({
                "InfluxDBv3Core": {
                    "execMemPoolBytes": { "absolute": 8000000000i64 },
                    "walReplayFailOnError": true,
                    "dataFusionRuntimeThreadPriority": -20
                }
            })),
            expected: json!({
                "InfluxDBv3Core": {
                    "execMemPoolBytes": { "absolute": 8000000000i64 },
                    "walReplayFailOnError": true,
                    "dataFusionRuntimeThreadPriority": -20
                }
            }),
        },
        Case {
            name: "the InfluxDBv3Enterprise family with its required members",
            requested: Some(json!({
                "InfluxDBv3Enterprise": {
                    "ingestQueryInstances": 2,
                    "queryOnlyInstances": 1,
                    "dedicatedCompactor": true,
                    "compactionRowLimit": 1000,
                    "replicationInterval": { "durationType": "seconds", "value": 30 }
                }
            })),
            expected: json!({
                "InfluxDBv3Enterprise": {
                    "ingestQueryInstances": 2,
                    "queryOnlyInstances": 1,
                    "dedicatedCompactor": true,
                    "compactionRowLimit": 1000,
                    "replicationInterval": { "durationType": "seconds", "value": 30 }
                }
            }),
        },
    ]
}

/// Exactly one family is selected, and it is one the union models.
fn assert_one_family(case: &str, parameters: &Value) {
    let object = parameters
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a union object, got {parameters}"));
    let selected: Vec<&String> = object.keys().collect();
    assert_eq!(
        selected.len(),
        1,
        "case {case:?}: a union carries exactly one member, got {parameters}"
    );
    let selected = selected[0].as_str();
    assert!(
        PARAMETER_FAMILIES.contains(&selected),
        "case {case:?}: {selected:?} is not a modelled parameter family, got {parameters}"
    );
}

#[tokio::test]
async fn every_group_reports_the_family_it_configures() {
    let router = router();

    for (index, case) in cases().into_iter().enumerate() {
        // A name per row, within `DbParameterGroupName` (length 3..64, pattern
        // `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$`), so the rows do not conflict.
        let mut request = json!({ "name": format!("ParamFamily{index}") });
        if let Some(requested) = &case.requested {
            request["parameters"] = requested.clone();
        }

        let created = ok(case.name, call(&router, "CreateDbParameterGroup", request).await);
        assert_eq!(
            created["parameters"], case.expected,
            "case {:?}: CreateDbParameterGroup must report the group's parameter family, \
             expected {}, got {}",
            case.name, case.expected, created["parameters"]
        );
        assert_one_family(case.name, &created["parameters"]);

        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a string id, got {created}", case.name));

        // The read reports the union the group was stored with, so the value was
        // persisted at create time rather than recomputed from the request.
        let fetched = ok(
            case.name,
            call(&router, "GetDbParameterGroup", json!({ "identifier": id })).await,
        );
        assert_eq!(
            fetched["parameters"], case.expected,
            "case {:?}: GetDbParameterGroup must report the stored parameter family, \
             expected {}, got {}",
            case.name, case.expected, fetched["parameters"]
        );

        // A read changes nothing a later caller sees.
        let again = ok(
            case.name,
            call(&router, "GetDbParameterGroup", json!({ "identifier": id })).await,
        );
        assert_eq!(
            again, fetched,
            "case {:?}: a second read must report the same record, got {again}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_default_family_is_reported_whatever_else_the_request_carries() {
    let router = router();
    // The rule is about the absent `parameters` member, not about the rest of the
    // request: every row below omits it and differs in what else it sets.
    let rows: Vec<(&str, Value)> = vec![
        ("name only", json!({ "name": "DefaultsOnlyName" })),
        (
            "a description as well",
            json!({ "name": "DefaultsDescribed", "description": "no family chosen" }),
        ),
        (
            "an empty description",
            json!({ "name": "DefaultsEmptyDescription", "description": "" }),
        ),
        (
            "tags as well",
            json!({ "name": "DefaultsTagged", "tags": { "team": "data", "env": "test" } }),
        ),
    ];

    for (case, request) in rows {
        let created = ok(case, call(&router, "CreateDbParameterGroup", request).await);
        assert_eq!(
            created["parameters"],
            default_family(),
            "case {case:?}: expected the default parameter family {}, got {}",
            default_family(),
            created["parameters"]
        );
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a string id, got {created}"));
        let fetched = ok(
            case,
            call(&router, "GetDbParameterGroup", json!({ "identifier": id })).await,
        );
        assert_eq!(
            fetched["parameters"],
            default_family(),
            "case {case:?}: the read must report the default family {}, got {}",
            default_family(),
            fetched["parameters"]
        );
    }
}
