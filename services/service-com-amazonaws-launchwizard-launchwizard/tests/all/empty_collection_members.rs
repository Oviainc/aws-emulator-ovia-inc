// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for ONE rule about every response this service writes: an output
//! member whose modelled map or list constrains the collection to at least one entry is
//! LEFT OUT when there is nothing to report, never serialised as an empty collection —
//! an empty value is outside the member's own declared bounds, and "nothing" is said by
//! the member's absence.
//!
//! The rule is asserted over a table rather than one call:
//!
//! * the SET of members it covers is read off `smithy-model/` (every structure member
//!   targeting a map or list with a length floor), so no member is asserted by hand and
//!   none is missed;
//! * the tag set it is driven over spans the counts the floor generalises over (none,
//!   one, several, the modelled maximum) and BOTH ways a resource comes to carry none
//!   (created without tags, and untagged after carrying some);
//! * both readers of the stored field (`ListTagsForResource` and `GetDeployment`) are
//!   asserted to answer the same way, and every read of the seeded catalogue is swept
//!   for an empty floored member too.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::deployment_lifecycle::required_specifications;
use crate::deployment_update::every_pattern;
use crate::harness::{catalogue, deployment_name, Harness};
use crate::resource_tagging::{
    as_map, deployment_arn, deployment_tags, listed_tags, model, tags_path, untag_path,
};

/// `Tags`' modelled entry ceiling, pinned to the model by
/// [`the_floors_this_suite_asserts_are_the_models`].
const MAX_TAGS: usize = 200;

/// The workload and pattern the tag rows are driven over; any catalogue entry serves,
/// and the catalogue-wide sweep below covers the rest.
const WORKLOAD: &str = "SAP";

/// Every structure MEMBER name, as it travels on the wire, whose target is a map or a
/// list the model gives a length floor of at least one entry: the members an empty
/// collection is not a legal value for.
///
/// Read off `smithy-model/` rather than written out here, so this suite cannot assert
/// the rule for a member that no longer carries a floor, nor miss one that gained it.
fn members_with_a_floor(model: &Value) -> BTreeSet<String> {
    let shapes = model["shapes"]
        .as_object()
        .expect("the model carries a shapes map");

    // The collection shapes whose own `@length` forbids zero entries.
    let floored: BTreeSet<&String> = shapes
        .iter()
        .filter(|(_, shape)| {
            matches!(shape["type"].as_str(), Some("map") | Some("list"))
                && shape["traits"]["smithy.api#length"]["min"]
                    .as_u64()
                    .is_some_and(|min| min >= 1)
        })
        .map(|(id, _)| id)
        .collect();

    let mut members = BTreeSet::new();
    for shape in shapes.values() {
        if shape["type"].as_str() != Some("structure") {
            continue;
        }
        let Some(declared) = shape["members"].as_object() else {
            continue;
        };
        for (name, member) in declared {
            let target = member["target"].as_str().unwrap_or_default().to_string();
            if !floored.contains(&target) {
                continue;
            }
            // `@jsonName` renames a member on the wire; without one the member name
            // travels as declared.
            let wire = member["traits"]["smithy.api#jsonName"]
                .as_str()
                .unwrap_or(name);
            members.insert(wire.to_string());
        }
    }
    members
}

/// Assert no floored member anywhere in `value` carries an empty collection.
fn no_empty_floored_member(case: &str, floored: &BTreeSet<String>, value: &Value) {
    match value {
        Value::Object(members) => {
            for (name, child) in members {
                let empty = match child {
                    Value::Object(map) => map.is_empty(),
                    Value::Array(items) => items.is_empty(),
                    _ => false,
                };
                assert!(
                    !(empty && floored.contains(name)),
                    "case {case:?}: member {name:?} is modelled with a floor of one entry, so \
                     nothing to report must be reported by leaving it out, got {child}"
                );
                no_empty_floored_member(case, floored, child);
            }
        }
        Value::Array(items) => {
            for item in items {
                no_empty_floored_member(case, floored, item);
            }
        }
        _ => {}
    }
}

#[test]
fn the_floors_this_suite_asserts_are_the_models() {
    let model = model();
    let floored = members_with_a_floor(&model);

    // The members the rows below drive the rule through must be among the ones the
    // model gives a floor, or those rows are asserting nothing.
    for member in ["tags", "specifications", "conditionals"] {
        assert!(
            floored.contains(member),
            "case {member:?}: the model must give this member a length floor, got {floored:?}"
        );
    }
    // And a member whose target declares NO floor must stay out of the set, so the
    // sweep cannot refuse an empty collection the model allows.
    for member in ["allowedValues", "deployments", "workloads", "deploymentEvents"] {
        assert!(
            !floored.contains(member),
            "case {member:?}: this member's target declares no floor, got {floored:?}"
        );
    }

    let ceiling = model["shapes"]["com.amazonaws.launchwizard#Tags"]["traits"]
        ["smithy.api#length"]["max"]
        .as_u64();
    assert_eq!(
        ceiling,
        Some(MAX_TAGS as u64),
        "Tags' modelled entry ceiling must be the one this suite's rows use, got {ceiling:?}"
    );
}

/// `count` distinct tags, in a map `Tags` accepts.
fn tag_map(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("Key{index}"), format!("value-{index}")))
        .collect()
}

/// Create a deployment carrying exactly `tags` — the member is OMITTED for an empty
/// map, because `Tags`' floor forbids an empty map on the way in just as it does on the
/// way out.
async fn create_carrying(
    harness: &Harness,
    case: &str,
    name: &str,
    pattern: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let mut body = json!({
        "workloadName": WORKLOAD,
        "deploymentPatternName": pattern,
        "name": deployment_name(name),
        "specifications": Value::Object(required_specifications(harness, WORKLOAD, pattern).await),
        "dryRun": false,
    });
    if !tags.is_empty() {
        body["tags"] = json!(tags);
    }
    let reply = harness.ok(case, "/createDeployment", body).await;
    reply["deploymentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeployment returned no id: {reply}"))
        .to_string()
}

/// Read both readers of the stored tag set and assert each reports `expected`: present
/// with those entries when there are any, ABSENT when there are none.
async fn both_readers_report(
    harness: &Harness,
    case: &str,
    floored: &BTreeSet<String>,
    id: &str,
    expected: &BTreeMap<String, String>,
) {
    let arn = deployment_arn(id);

    let listed = harness
        .expect_ok(case, "GET", &tags_path(&arn), None)
        .await;
    no_empty_floored_member(case, floored, &listed);
    let got = harness.ok(case, "/getDeployment", json!({"deploymentId": id})).await;
    no_empty_floored_member(case, floored, &got);

    if expected.is_empty() {
        assert!(
            listed.get("tags").is_none(),
            "case {case:?}: ListTagsForResource must leave `tags` out for a resource with no \
             tags, got {listed}"
        );
        assert!(
            got["deployment"].get("tags").is_none(),
            "case {case:?}: GetDeployment must leave `tags` out for a deployment with no tags, \
             got {got}"
        );
    } else {
        let reported = listed
            .get("tags")
            .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource reported no tags: {listed}"));
        assert_eq!(
            &as_map(case, reported),
            expected,
            "case {case:?}: ListTagsForResource must report the stored tag set, got {reported}"
        );
        let carried = got["deployment"]
            .get("tags")
            .unwrap_or_else(|| panic!("case {case:?}: GetDeployment reported no tags: {got}"));
        assert_eq!(
            &as_map(case, carried),
            expected,
            "case {case:?}: GetDeployment must report the same set, got {carried}"
        );
    }

    // Both readers of one stored field, the same answer either way.
    assert_eq!(
        listed_tags(harness, case, &arn).await,
        deployment_tags(harness, case, id).await,
        "case {case:?}: the two readers of the deployment's tags must agree"
    );

    // `specifications` has a floor of its own and a deployment always carries settings,
    // so it is reported — the rule omits a member only when there is nothing to report.
    let specifications = got["deployment"]
        .get("specifications")
        .unwrap_or_else(|| panic!("case {case:?}: GetDeployment reported no specifications: {got}"));
    assert!(
        specifications.as_object().is_some_and(|map| !map.is_empty()),
        "case {case:?}: a deployment's settings are never empty, so the member is reported, \
         got {specifications}"
    );
}

#[tokio::test]
async fn a_member_with_a_modelled_floor_is_absent_rather_than_empty() {
    let harness = Harness::new();
    let model = model();
    let floored = members_with_a_floor(&model);
    let pattern = crate::deployment_lifecycle::first_pattern(WORKLOAD);

    // Rows differ along the COUNT the floor generalises over: none at all, the floor
    // itself, several, and the modelled ceiling.
    for count in [0usize, 1, 3, MAX_TAGS] {
        let case = format!("a deployment created carrying {count} tags");
        let mut expected = tag_map(count);
        let id = create_carrying(
            &harness,
            &case,
            &format!("floor-{count}"),
            pattern,
            &expected,
        )
        .await;
        both_readers_report(&harness, &case, &floored, &id, &expected).await;

        let arn = deployment_arn(&id);
        if count > 0 {
            // Untagging down to nothing is the OTHER way a resource comes to carry no
            // tags, and it must read exactly like one created without any.
            let keys: Vec<String> = expected.keys().cloned().collect();
            harness
                .expect_ok(&case, "DELETE", &untag_path(&arn, &keys), None)
                .await;
            expected.clear();
            let emptied = format!("{case}, then untagged down to nothing");
            both_readers_report(&harness, &emptied, &floored, &id, &expected).await;
        }

        // And a single tag written onto the emptied resource brings the member back:
        // the member's absence is the tag set's emptiness, not a one-way switch.
        let written = BTreeMap::from([("Owner".to_string(), "emulator-tests".to_string())]);
        harness
            .expect_ok(&case, "POST", &tags_path(&arn), Some(&json!({"tags": written})))
            .await;
        let retagged = format!("{case}, then tagged with one key");
        both_readers_report(&harness, &retagged, &floored, &id, &written).await;
    }
}

#[tokio::test]
async fn no_read_of_the_catalogue_reports_an_empty_floored_member() {
    let harness = Harness::new();
    let model = model();
    let floored = members_with_a_floor(&model);

    // Every catalogue read, over the WHOLE seeded catalogue rather than one entry: the
    // specification schema and its per-field conditionals carry floors of their own.
    for workload in catalogue(&harness).await {
        let case = format!("GetWorkload {workload}");
        let body = harness
            .ok(&case, "/getWorkload", json!({"workloadName": workload}))
            .await;
        no_empty_floored_member(&case, &floored, &body);

        let case = format!("ListWorkloadDeploymentPatterns {workload}");
        let body = harness
            .ok(
                &case,
                "/listWorkloadDeploymentPatterns",
                json!({"workloadName": workload}),
            )
            .await;
        no_empty_floored_member(&case, &floored, &body);
    }

    for (workload, pattern) in every_pattern(&harness).await {
        let case = format!("GetWorkloadDeploymentPattern {workload}/{pattern}");
        let body = harness
            .ok(
                &case,
                "/getWorkloadDeploymentPattern",
                json!({"workloadName": workload, "deploymentPatternName": pattern}),
            )
            .await;
        no_empty_floored_member(&case, &floored, &body);
        // The schema itself is reported, because a pattern always publishes fields.
        let fields = body["workloadDeploymentPattern"]["specifications"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no specification schema reported: {body}"));
        assert!(
            !fields.is_empty(),
            "case {case:?}: a published pattern's schema is never empty, got {body}"
        );
    }

    // And the deployment-side reads, including the ones that report nothing at all.
    let case = "ListDeployments over an empty store";
    let body = harness.ok(case, "/listDeployments", json!({})).await;
    no_empty_floored_member(case, &floored, &body);

    let case = "ListWorkloads";
    let body = harness.ok(case, "/listWorkloads", json!({})).await;
    no_empty_floored_member(case, &floored, &body);
}
