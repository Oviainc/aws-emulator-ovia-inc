// SPDX-License-Identifier: CC0-1.0

//! The specification schema a deployment pattern publishes is THAT pattern's own
//! contract, driven over HTTP through the same router the production emulator serves
//! with.
//!
//! The rule under test: `GetWorkloadDeploymentPattern` reports the schema of the pattern
//! it was asked for — which fields that pattern reads, and for each field whether it is
//! required, which values it allows and what it is conditional on — so two patterns of
//! one workload publish different schemas, and `CreateDeployment` judges a settings map
//! against the schema of the pattern it names rather than against a per-workload one.
//!
//! Every assertion is made over a TABLE spanning the class rather than over one call:
//! every workload the catalogue holds and every pair of patterns within it, a setting
//! published by one pattern of a workload and not by its sibling (accepted for the one,
//! refused for the other), and each documented `CreateDeployment` request the model
//! itself carries, submitted as written. No field name is written by hand: the schema rows are read out of
//! what the service publishes, and the documented rows out of `smithy-model/`.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::deployment_lifecycle::get;
use crate::deployment_update::{conforming_specifications, error_message, pattern_data, schema_fields};
use crate::harness::{catalogue, deployment_name, Harness};
use crate::resource_tagging::model;

/// The wire error code and HTTP status a settings map the pattern's schema rejects
/// carries (`smithy_query(query="shape",shape="ValidationException")`: `errorKind:
/// client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// Every `deploymentPatternName` the catalogue offers for `workload`, in the order it
/// reports them.
async fn patterns_of(harness: &Harness, workload: &str) -> Vec<String> {
    let body = harness
        .ok(
            workload,
            "/listWorkloadDeploymentPatterns",
            json!({"workloadName": workload}),
        )
        .await;
    body["workloadDeploymentPatterns"]
        .as_array()
        .unwrap_or_else(|| panic!("case {workload:?}: no patterns reported: {body}"))
        .iter()
        .map(|summary| {
            summary["deploymentPatternName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {workload:?}: a pattern carries no name"))
                .to_string()
        })
        .collect()
}

/// The names of every field a published schema carries.
fn published_names(case: &str, data: &Value) -> BTreeSet<String> {
    schema_fields(case, data)
        .iter()
        .map(|field| {
            field["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a schema field carries no name"))
                .to_string()
        })
        .collect()
}

/// The names of the fields a published schema marks `required: "Yes"`.
fn required_names(case: &str, data: &Value) -> BTreeSet<String> {
    schema_fields(case, data)
        .iter()
        .filter(|field| field["required"].as_str() == Some("Yes"))
        .map(|field| {
            field["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a schema field carries no name"))
                .to_string()
        })
        .collect()
}

/// A `CreateDeployment` request body over an explicit settings map. `name` is the
/// case's descriptive label, carried into a name the service's own name rule accepts by
/// [`deployment_name`].
fn create_request(
    workload: &str,
    pattern: &str,
    name: &str,
    specifications: &Map<String, Value>,
) -> Value {
    json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
        "name": deployment_name(name),
        "specifications": Value::Object(specifications.clone()),
    })
}

#[tokio::test]
async fn every_pattern_of_a_workload_publishes_its_own_specification_schema() {
    let harness = Harness::new();
    let workloads = catalogue(&harness).await;
    assert!(
        workloads.len() >= 2,
        "the catalogue must hold enough workloads to span the rule, got {workloads:?}"
    );

    for workload in workloads {
        let patterns = patterns_of(&harness, &workload).await;
        assert!(
            patterns.len() >= 2,
            "case {workload:?}: the rule is about telling two patterns of one workload \
             apart, so the catalogue must offer at least two, got {patterns:?}"
        );

        // One row per pattern: the schema it publishes, with the two sets the rule is
        // about read off it.
        let mut schemas: Vec<(String, BTreeSet<String>, BTreeSet<String>, Value)> = Vec::new();
        for pattern in &patterns {
            let case = format!("{workload}/{pattern}");
            let data = pattern_data(&harness, &case, &workload, pattern).await;
            let published = published_names(&case, &data);
            let required = required_names(&case, &data);
            assert!(
                !required.is_empty(),
                "case {case:?}: a pattern must state the settings a deployment of it \
                 cannot be created without, got {required:?}"
            );
            assert!(
                required.is_subset(&published),
                "case {case:?}: every required setting must be one the pattern publishes, \
                 got required {required:?} over published {published:?}"
            );
            schemas.push((pattern.clone(), published, required, data));
        }

        // Pairwise over the workload's patterns: no two publish the same schema, and the
        // difference is in WHAT the pattern asks for — the fields it reads or the ones it
        // insists on — rather than only in the prose beside them.
        for (left_index, (left, left_published, left_required, left_data)) in
            schemas.iter().enumerate()
        {
            for (right, right_published, right_required, right_data) in
                schemas.iter().skip(left_index + 1)
            {
                let case = format!("{workload}: {left} vs {right}");
                assert_ne!(
                    left_data["specifications"], right_data["specifications"],
                    "case {case:?}: two patterns of one workload must not publish a \
                     byte-identical schema, got {}",
                    left_data["specifications"]
                );
                assert!(
                    left_published != right_published || left_required != right_required,
                    "case {case:?}: the schemas must differ in the fields the pattern reads \
                     or the ones it requires, got published {left_published:?} and \
                     {right_published:?} with required {left_required:?} and \
                     {right_required:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_setting_one_pattern_publishes_is_refused_for_the_sibling_that_does_not() {
    let harness = Harness::new();
    for workload in catalogue(&harness).await {
        let patterns = patterns_of(&harness, &workload).await;

        // The first (pattern, sibling, setting) of this workload where one pattern reads
        // a setting the other does not — the pair the rule is observable through.
        let mut rows: Vec<(String, String, String)> = Vec::new();
        for owner in &patterns {
            let owner_case = format!("{workload}/{owner}");
            let owner_data = pattern_data(&harness, &owner_case, &workload, owner).await;
            let owner_published = published_names(&owner_case, &owner_data);
            for sibling in &patterns {
                if sibling == owner {
                    continue;
                }
                let sibling_case = format!("{workload}/{sibling}");
                let sibling_data = pattern_data(&harness, &sibling_case, &workload, sibling).await;
                let sibling_published = published_names(&sibling_case, &sibling_data);
                if let Some(setting) = owner_published.difference(&sibling_published).next() {
                    rows.push((owner.clone(), sibling.clone(), setting.clone()));
                }
            }
        }
        assert!(
            !rows.is_empty(),
            "case {workload:?}: some pattern of a workload must read a setting another of \
             its patterns does not, or the two cannot be told apart"
        );

        for (row, (owner, sibling, setting)) in rows.into_iter().enumerate() {
            // The owner publishes the setting, so a create for it that carries the whole
            // published schema is accepted and stored.
            let owner_case = format!("{workload}/{owner} carrying {setting}");
            let owner_data = pattern_data(&harness, &owner_case, &workload, &owner).await;
            let owner_map = conforming_specifications(&owner_case, &owner_data, true);
            assert!(
                owner_map.contains_key(&setting),
                "case {owner_case:?}: the owner's conforming map must carry the setting it \
                 publishes, got {owner_map:?}"
            );
            let created = harness
                .ok(
                    &owner_case,
                    "/createDeployment",
                    create_request(
                        &workload,
                        &owner,
                        &format!("own-{workload}-{row}"),
                        &owner_map,
                    ),
                )
                .await;
            let id = created["deploymentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {owner_case:?}: no deploymentId in {created}"))
                .to_string();
            let stored = get(&harness, &owner_case, &id).await;
            assert_eq!(
                stored["specifications"],
                Value::Object(owner_map),
                "case {owner_case:?}: the settings the pattern's own schema accepts must come \
                 back as stored, got {}",
                stored["specifications"]
            );

            // The sibling does not publish it, so the SAME setting — on an otherwise
            // conforming map for the sibling — is refused, and the refusal names it.
            let sibling_case = format!("{workload}/{sibling} carrying {setting}");
            let sibling_data = pattern_data(&harness, &sibling_case, &workload, &sibling).await;
            let mut sibling_map = conforming_specifications(&sibling_case, &sibling_data, true);
            sibling_map.insert(setting.clone(), json!("a-value-for-another-pattern"));
            let body = harness
                .refused(
                    &sibling_case,
                    "/createDeployment",
                    create_request(
                        &workload,
                        &sibling,
                        &format!("sibling-{workload}-{row}"),
                        &sibling_map,
                    ),
                    STATUS,
                    CODE,
                )
                .await;
            let message = error_message(&body);
            assert!(
                message.contains(&setting),
                "case {sibling_case:?}: the refusal must name the setting {setting:?} the \
                 pattern does not publish, got {message:?}"
            );
        }
    }
}

/// Every `(workloadName, deploymentPatternName, specifications)` the model's own
/// `CreateDeployment` examples submit: the documented requests a client copies. Source:
/// the `smithy.api#examples` trait on `CreateDeployment` in `smithy-model/`.
fn documented_create_examples() -> Vec<(String, String, Map<String, Value>)> {
    let model = model();
    let examples = model["shapes"]["com.amazonaws.launchwizard#CreateDeployment"]["traits"]
        ["smithy.api#examples"]
        .as_array()
        .expect("the model's CreateDeployment carries documented examples")
        .clone();
    examples
        .iter()
        .filter_map(|example| {
            let input = &example["input"];
            Some((
                input["workloadName"].as_str()?.to_string(),
                input["deploymentPatternName"].as_str()?.to_string(),
                input["specifications"].as_object()?.clone(),
            ))
        })
        .collect()
}

#[tokio::test]
async fn the_settings_the_models_own_create_example_submits_are_published_and_accepted() {
    let harness = Harness::new();
    let examples = documented_create_examples();
    assert!(
        !examples.is_empty(),
        "the model must carry a documented CreateDeployment request to check the published \
         schema against"
    );

    for (row, (workload, pattern, documented)) in examples.into_iter().enumerate() {
        let case = format!("{workload}/{pattern} documented example {row}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let published = published_names(&case, &data);

        // Every setting the documented request submits is one the named pattern
        // publishes, so a client that copies it is not refused for a key the emulator
        // never reports.
        for setting in documented.keys() {
            assert!(
                published.contains(setting),
                "case {case:?}: the pattern must publish the documented setting \
                 {setting:?}, got {published:?}"
            );
        }

        // And the request is served AS WRITTEN: a caller that copies the documented
        // settings is not refused for a field the pattern requires but the documented
        // request does not carry.
        let required = required_names(&case, &data);
        let documented_keys: BTreeSet<String> = documented.keys().cloned().collect();
        assert!(
            required.is_subset(&documented_keys),
            "case {case:?}: a pattern the model publishes a request for must require \
             nothing that request omits, got required {required:?} over documented \
             {documented_keys:?}"
        );
        let specifications = documented.clone();
        let created = harness
            .ok(
                &case,
                "/createDeployment",
                create_request(
                    &workload,
                    &pattern,
                    &format!("documented-{row}"),
                    &specifications,
                ),
            )
            .await;
        let id = created["deploymentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no deploymentId in {created}"))
            .to_string();
        let stored = get(&harness, &case, &id).await;
        assert_eq!(
            stored["specifications"],
            Value::Object(specifications),
            "case {case:?}: the documented settings must come back as stored, got {}",
            stored["specifications"]
        );
    }
}
