// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Launch Wizard workload deployment-pattern catalogue:
//! `ListWorkloadDeploymentPatterns` and the `GetWorkloadDeploymentPattern` detail view
//! it must agree with, driven over HTTP through the same router the production
//! emulator serves with (the harness in `deployment_lifecycle`).
//!
//! Every assertion here is about a RULE rather than one recorded call, so each one is
//! exercised over a table of inputs: EVERY workload of the seeded catalogue and every
//! pattern of each (not just the `SAP`/`SapHanaSingle` entry the issue cited), every
//! page size from one to one-past-the-end, each published bound of `maxResults` and
//! one past it, and both arms of the not-found contract (an unknown workload and an
//! unknown pattern of a known workload).

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::WORKLOAD_DEPLOYMENT_PATTERN_STATUS_VALUES;

// The HTTP harness, the catalogue reader and the pagination walker are shared with the
// other suites in this crate rather than copied — see `tests/all/harness.rs`.
use crate::deployment_lifecycle::get;
use crate::harness::{catalogue, deployment_name, walk_pages, Harness};

/// The two values `DeploymentSpecificationsField$required` carries: the model types
/// the member as a plain `String` and documents it as "Indicates if the deployment
/// specification is required", and the vocabulary is the `Yes`/`No` pair the model's
/// own `CreateDeployment` example submits for its boolean specifications.
const REQUIRED_VALUES: [&str; 2] = ["Yes", "No"];

/// The comparators a `DeploymentConditionalField` may use. Source: that member's doc
/// in the model — "Valid values: `Equal | NotEqual`".
const COMPARATOR_VALUES: [&str; 2] = ["Equal", "NotEqual"];

/// The members a `WorkloadDeploymentPatternDataSummary` and a
/// `WorkloadDeploymentPatternData` both carry — everything the detail view adds beyond
/// them is `specifications`.
const SHARED_MEMBERS: [&str; 7] = [
    "workloadName",
    "deploymentPatternName",
    "workloadVersionName",
    "deploymentPatternVersionName",
    "displayName",
    "description",
    "status",
];

/// True when `name` satisfies `DeploymentPatternName`: len 1..256, pattern
/// `^[A-Za-z0-9][a-zA-Z0-9-]*$`.
fn is_deployment_pattern_name(name: &str) -> bool {
    (1..=256).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// True when `name` satisfies `WorkloadVersionName` / `DeploymentPatternVersionName`:
/// len 5..30, pattern `^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$`.
fn is_version_name(name: &str) -> bool {
    let dotted = {
        let parts: Vec<&str> = name.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    };
    let dashed = name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    (5..=30).contains(&name.len()) && (dotted || dashed)
}

/// True when `key` is a usable `DeploymentSpecifications` KEY: len 3..256, pattern
/// `^[a-zA-Z0-9-:]+$`.
fn is_specification_key(key: &str) -> bool {
    (3..=256).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':')
}

/// Every `workloadName` of the seeded catalogue, in the order `ListWorkloads` pages
/// them — the table every test below runs over.
async fn workload_names(harness: &Harness) -> Vec<String> {
    let names = catalogue(harness).await;
    assert!(
        names.len() >= 2,
        "the catalogue must be seeded with enough workloads to span the rule, got {names:?}"
    );
    names
}

/// One unpaginated page of `ListWorkloadDeploymentPatterns` for `workload`.
async fn list_patterns(harness: &Harness, case: &str, workload: &str) -> Vec<Value> {
    let body = harness
        .ok(
            case,
            "/listWorkloadDeploymentPatterns",
            json!({"workloadName": workload}),
        )
        .await;
    assert!(
        body.get("nextToken").is_none() || body["nextToken"].is_null(),
        "case {case:?}: a page that holds every pattern carries no nextToken, got {}",
        body["nextToken"]
    );
    body["workloadDeploymentPatterns"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListWorkloadDeploymentPatterns reported no list: {body}")
        })
        .clone()
}

/// The `WorkloadDeploymentPatternData` `GetWorkloadDeploymentPattern` reports.
async fn get_pattern(harness: &Harness, case: &str, workload: &str, pattern: &str) -> Value {
    let body = harness
        .ok(
            case,
            "/getWorkloadDeploymentPattern",
            json!({"workloadName": workload, "deploymentPatternName": pattern}),
        )
        .await;
    let data = body["workloadDeploymentPattern"].clone();
    assert!(
        data.is_object(),
        "case {case:?}: GetWorkloadDeploymentPattern must report a pattern, got {body}"
    );
    data
}

/// The `deploymentPatternName` of each reported summary, in the order reported.
fn pattern_names(patterns: &[Value]) -> Vec<String> {
    patterns
        .iter()
        .map(|pattern| {
            pattern["deploymentPatternName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_seeded_workload_reports_the_patterns_it_offers() {
    let harness = Harness::new();
    for workload in workload_names(&harness).await {
        let case = format!("listWorkloadDeploymentPatterns workloadName={workload}");
        let patterns = list_patterns(&harness, &case, &workload).await;
        assert!(
            !patterns.is_empty(),
            "case {case:?}: a seeded workload must offer at least one deployment pattern"
        );

        for pattern in &patterns {
            let name = pattern["deploymentPatternName"].as_str().unwrap_or_default();
            assert!(
                is_deployment_pattern_name(name),
                "case {case:?}: {name:?} is not a DeploymentPatternName (len 1..256, \
                 ^[A-Za-z0-9][a-zA-Z0-9-]*$)"
            );
            assert_eq!(
                pattern["workloadName"],
                json!(workload),
                "case {case:?}: pattern {name:?} must name the workload it was listed for, got {}",
                pattern["workloadName"]
            );
            for member in ["displayName", "description"] {
                assert!(
                    pattern[member].as_str().is_some_and(|text| !text.is_empty()),
                    "case {case:?}: pattern {name:?} must carry a {member}, got {}",
                    pattern[member]
                );
            }
            let status = pattern["status"].as_str().unwrap_or_default();
            assert!(
                WORKLOAD_DEPLOYMENT_PATTERN_STATUS_VALUES.contains(&status),
                "case {case:?}: pattern {name:?} status {status:?} is not a \
                 WorkloadDeploymentPatternStatus value"
            );
            for member in ["workloadVersionName", "deploymentPatternVersionName"] {
                let version = pattern[member].as_str().unwrap_or_default();
                assert!(
                    is_version_name(version),
                    "case {case:?}: pattern {name:?} {member} {version:?} is not a version name \
                     (len 5..30, ^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\\d+\\.\\d+\\.\\d+))$)"
                );
            }
        }

        // The patterns are reported in a stable, defined order, so paging over them is
        // well defined.
        let names = pattern_names(&patterns);
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "case {case:?}: patterns must be reported in deploymentPatternName order"
        );

        // A read changes nothing a later caller sees: the same request answers the
        // same page.
        let again = list_patterns(&harness, &case, &workload).await;
        assert_eq!(
            again, patterns,
            "case {case:?}: listing patterns must not change what a later caller sees"
        );
    }
}

#[tokio::test]
async fn patterns_page_at_every_size_and_a_bad_page_request_is_refused() {
    let harness = Harness::new();
    for workload in workload_names(&harness).await {
        let base = json!({"workloadName": workload});
        let whole = pattern_names(&list_patterns(&harness, &workload, &workload).await);

        // Every page size from one item to one past the end yields the same list in
        // the same order, with a token only while items remain (walk_pages asserts the
        // token contract).
        for page_size in 1..=whole.len() + 1 {
            let case = format!("{workload} maxResults={page_size}");
            let paged = walk_pages(
                &harness,
                &case,
                "/listWorkloadDeploymentPatterns",
                &base,
                "workloadDeploymentPatterns",
                page_size,
            )
            .await;
            assert_eq!(
                pattern_names(&paged),
                whole,
                "case {case:?}: paging must yield every pattern once, in order"
            );
        }

        // `maxResults` is @range(min: 1, max: 100): each published bound is accepted
        // and one past it on either side is refused.
        for max_results in [1, 100] {
            let case = format!("{workload} maxResults={max_results} (published bound)");
            let body = harness
                .ok(
                    &case,
                    "/listWorkloadDeploymentPatterns",
                    json!({"workloadName": workload, "maxResults": max_results}),
                )
                .await;
            let page = body["workloadDeploymentPatterns"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no list in {body}"))
                .clone();
            assert!(
                page.len() <= max_results as usize && !page.is_empty(),
                "case {case:?}: a page must hold between one and maxResults items, got {}",
                page.len()
            );
        }
        for max_results in [0, 101] {
            let case = format!("{workload} maxResults={max_results} (past the bound)");
            harness
                .refused(
                    &case,
                    "/listWorkloadDeploymentPatterns",
                    json!({"workloadName": workload, "maxResults": max_results}),
                    400,
                    "ValidationException",
                )
                .await;
        }

        // A token this service never issued names no position in the results, so it is
        // a client error rather than a silent restart from the top.
        for token in ["not-a-token", "6e6f7065"] {
            let case = format!("{workload} nextToken={token}");
            harness
                .refused(
                    &case,
                    "/listWorkloadDeploymentPatterns",
                    json!({"workloadName": workload, "nextToken": token}),
                    400,
                    "ValidationException",
                )
                .await;
        }
    }

    // A workloadName outside the modelled constraints is refused at the boundary:
    // len 1..100, pattern ^[A-Za-z][a-zA-Z0-9-_]*$.
    for bad_name in ["9StartsWithADigit", "has space", &"W".repeat(101)] {
        let case = format!("workloadName={bad_name:?}");
        harness
            .refused(
                &case,
                "/listWorkloadDeploymentPatterns",
                json!({"workloadName": bad_name}),
                400,
                "ValidationException",
            )
            .await;
    }

    // A well-formed name the catalogue does not hold is a 404 whose message does not
    // report the name back.
    for unknown in ["NoSuchWorkloadXyz", "SAPx", "Sql-Server"] {
        let case = format!("unknown workloadName={unknown}");
        let body = harness
            .refused(
                &case,
                "/listWorkloadDeploymentPatterns",
                json!({"workloadName": unknown}),
                404,
                "ResourceNotFoundException",
            )
            .await;
        let message = body["message"].as_str().unwrap_or_default().to_string();
        assert!(
            !message.is_empty() && !message.contains(unknown),
            "case {case:?}: the not-found message must not echo the requested name, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn the_listed_summary_matches_the_get_detail_for_every_pattern() {
    let harness = Harness::new();
    for workload in workload_names(&harness).await {
        for summary in list_patterns(&harness, &workload, &workload).await {
            let name = summary["deploymentPatternName"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let case = format!("{workload}/{name}");
            let data = get_pattern(&harness, &case, &workload, &name).await;

            // Both views report the SAME record, so every member they share agrees.
            for member in SHARED_MEMBERS {
                assert_eq!(
                    data[member], summary[member],
                    "case {case:?}: {member} must agree between the list and the get, got {} \
                     and {}",
                    summary[member], data[member]
                );
            }

            // Only the detail view carries the specification schema a
            // `CreateDeployment` for this pattern fills in.
            assert!(
                summary.get("specifications").is_none() || summary["specifications"].is_null(),
                "case {case:?}: a summary carries no specifications, got {}",
                summary["specifications"]
            );
            let fields = data["specifications"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no specifications in {data}"))
                .clone();
            // `DeploymentSpecificationsData` is @length(min: 1, max: 100).
            assert!(
                (1..=100).contains(&fields.len()),
                "case {case:?}: the schema must hold 1..100 fields, got {}",
                fields.len()
            );
            let field_names: Vec<String> = fields
                .iter()
                .map(|field| field["name"].as_str().unwrap_or_default().to_string())
                .collect();
            for field in &fields {
                let field_name = field["name"].as_str().unwrap_or_default();
                assert!(
                    is_specification_key(field_name),
                    "case {case:?}: field name {field_name:?} is not a DeploymentSpecifications \
                     key (len 3..256, ^[a-zA-Z0-9-:]+$)"
                );
                assert!(
                    field["description"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "case {case:?}: field {field_name:?} must carry a description, got {}",
                    field["description"]
                );
                let required = field["required"].as_str().unwrap_or_default();
                assert!(
                    REQUIRED_VALUES.contains(&required),
                    "case {case:?}: field {field_name:?} required {required:?} is not one of \
                     {REQUIRED_VALUES:?}"
                );
                if !field["allowedValues"].is_null() {
                    let allowed = field["allowedValues"]
                        .as_array()
                        .unwrap_or_else(|| panic!("case {case:?}: allowedValues is not a list"));
                    assert!(
                        !allowed.is_empty()
                            && allowed.iter().all(|value| value
                                .as_str()
                                .is_some_and(|v| (1..=1500).contains(&v.len()))),
                        "case {case:?}: field {field_name:?} allowedValues must be a non-empty \
                         list of ValueStrings, got {:?}",
                        field["allowedValues"]
                    );
                }
                if !field["conditionals"].is_null() {
                    let conditionals = field["conditionals"]
                        .as_array()
                        .unwrap_or_else(|| panic!("case {case:?}: conditionals is not a list"));
                    // `SpecificationsConditionalData` is @length(min: 1, max: 5).
                    assert!(
                        (1..=5).contains(&conditionals.len()),
                        "case {case:?}: field {field_name:?} must hold 1..5 conditionals, got {}",
                        conditionals.len()
                    );
                    for conditional in conditionals {
                        let on = conditional["name"].as_str().unwrap_or_default();
                        assert!(
                            field_names.iter().any(|other| other == on),
                            "case {case:?}: field {field_name:?} is conditional on {on:?}, which \
                             is not a field of the same schema: {field_names:?}"
                        );
                        let comparator = conditional["comparator"].as_str().unwrap_or_default();
                        assert!(
                            COMPARATOR_VALUES.contains(&comparator),
                            "case {case:?}: conditional comparator {comparator:?} is not one of \
                             {COMPARATOR_VALUES:?}"
                        );
                        assert!(
                            conditional["value"]
                                .as_str()
                                .is_some_and(|value| !value.is_empty()),
                            "case {case:?}: conditional on {on:?} must carry a value, got {}",
                            conditional["value"]
                        );
                    }
                }
            }

            // Reading the detail twice agrees, and changes nothing.
            let again = get_pattern(&harness, &case, &workload, &name).await;
            assert_eq!(
                again, data,
                "case {case:?}: GetWorkloadDeploymentPattern must be a pure read"
            );
        }

        // A pattern the workload does not offer is a 404 that does not report the
        // probed name back — the same contract as an unknown workload.
        for unknown in ["NoSuchPattern", "SapHanaSingleX"] {
            let case = format!("{workload}/{unknown} (unknown pattern)");
            let body = harness
                .refused(
                    &case,
                    "/getWorkloadDeploymentPattern",
                    json!({"workloadName": workload, "deploymentPatternName": unknown}),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
            let message = body["message"].as_str().unwrap_or_default().to_string();
            assert!(
                !message.is_empty() && !message.contains(unknown),
                "case {case:?}: the not-found message must not echo the requested pattern, got \
                 {message:?}"
            );
        }
    }

    // And an unknown workload is a 404 whichever pattern is asked for.
    harness
        .refused(
            "unknown workload / known pattern name",
            "/getWorkloadDeploymentPattern",
            json!({"workloadName": "NoSuchWorkloadXyz", "deploymentPatternName": "SapHanaSingle"}),
            404,
            "ResourceNotFoundException",
        )
        .await;
}

#[tokio::test]
async fn the_reported_specification_schema_drives_a_create_deployment() {
    let harness = Harness::new();
    for workload in workload_names(&harness).await {
        for summary in list_patterns(&harness, &workload, &workload).await {
            let pattern = summary["deploymentPatternName"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let case = format!("{workload}/{pattern}");
            let data = get_pattern(&harness, &case, &workload, &pattern).await;

            // Fill in exactly what the reported schema asks for: every required field,
            // and for a field that publishes allowedValues one of those values.
            let mut specifications = serde_json::Map::new();
            for field in data["specifications"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no specifications in {data}"))
            {
                let name = field["name"].as_str().unwrap_or_default().to_string();
                let allowed = field["allowedValues"]
                    .as_array()
                    .and_then(|values| values.first())
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let required = field["required"].as_str() == Some("Yes");
                match (required, allowed) {
                    (_, Some(value)) => {
                        specifications.insert(name, json!(value));
                    }
                    (true, None) => {
                        specifications.insert(name.clone(), json!(format!("value-for-{name}")));
                    }
                    (false, None) => {}
                }
            }
            assert!(
                !specifications.is_empty(),
                "case {case:?}: the schema must ask for at least one specification"
            );

            let name = deployment_name(&format!("lw-{workload}-{pattern}"));
            let created = harness
                .ok(
                    &case,
                    "/createDeployment",
                    json!({
                        "workloadName": workload,
                        "deploymentPatternName": pattern,
                        "name": name,
                        "specifications": Value::Object(specifications.clone()),
                    }),
                )
                .await;
            let id = created["deploymentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no deploymentId in {created}"))
                .to_string();

            // Read it back: the schema the catalogue reported is the schema a
            // deployment of that pattern stores.
            let deployment = get(&harness, &case, &id).await;
            assert_eq!(
                deployment["specifications"],
                Value::Object(specifications),
                "case {case:?}: the submitted specifications must come back as stored, got {}",
                deployment["specifications"]
            );
            assert_eq!(
                deployment["patternName"],
                json!(pattern),
                "case {case:?}: the deployment must name the pattern it was created from, got {}",
                deployment["patternName"]
            );
            assert_eq!(
                deployment["workloadName"],
                json!(workload),
                "case {case:?}: the deployment must name its workload, got {}",
                deployment["workloadName"]
            );

            // The same pattern, with a `specifications` map that carries nothing, is
            // refused rather than stored: `DeploymentSpecifications` is
            // @length(min: 1, max: 100), so a deployment always fills in part of the
            // schema reported above.
            harness
                .refused(
                    &format!("{case} with an empty specifications map"),
                    "/createDeployment",
                    json!({
                        "workloadName": workload,
                        "deploymentPatternName": pattern,
                        "name": format!("{name}Bad"),
                        "specifications": json!({}),
                    }),
                    400,
                    "ValidationException",
                )
                .await;
        }
    }
}
