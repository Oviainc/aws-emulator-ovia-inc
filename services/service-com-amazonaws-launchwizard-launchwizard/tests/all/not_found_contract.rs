// SPDX-License-Identifier: CC0-1.0

//! Cover for Launch Wizard's resource-lookup contract, driven over HTTP through the
//! same router the production emulator serves with.
//!
//! The rule under test: a lookup that resolves no resource answers
//! `ResourceNotFoundException` (HTTP 404) with a FIXED message determined by the KIND
//! of resource looked up and never by the identifier the request carried — and every
//! modelled lookup operation answers that error rather than HTTP 501.
//!
//! So every assertion is made over a TABLE of identifiers spanning the shapes'
//! published bounds and forms (each identifier the issue recorded is one row of its
//! table), and the message is required to be the same string for every row of a kind.

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::WORKLOAD_STATUS_VALUES;

use crate::deployment_lifecycle::{create, create_body, first_pattern};
use crate::harness::{catalogue, Harness};

/// The fixed message a DEPLOYMENT lookup failure carries. Launch Wizard's recorded
/// wording; mirrored by `state::DEPLOYMENT_NOT_FOUND_MESSAGE`.
const DEPLOYMENT_NOT_FOUND: &str = "The specified deployment does not exist.";

/// The fixed message a WORKLOAD lookup failure carries. Launch Wizard's recorded
/// wording; mirrored by `state::WORKLOAD_NOT_FOUND_MESSAGE`.
const WORKLOAD_NOT_FOUND: &str =
    "The specified workload does not exist or insufficient permissions.";

/// The `message` an error body carries (restJson1 serialises the modelled `message`
/// member; the capitalised spelling is accepted so the assertion is about the text).
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Require `body`'s message to be exactly `expected` and to render none of the
/// identifier the caller asked about.
fn assert_fixed_message(case: &str, body: &Value, expected: &str, probe: &str) {
    let actual = error_message(body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the fixed message {expected:?}, got {actual:?} (body {body})"
    );
    // A one- or two-character identifier can occur in ordinary English by accident, so
    // the echo probe is only meaningful for a longer one.
    if probe.len() >= 3 {
        assert!(
            !actual.contains(probe),
            "case {case:?}: the message must not render the requested identifier \
             {probe:?}, got {actual:?}"
        );
    }
}

/// Well-formed `deploymentId`s no deployment has: `DeploymentId` is len 2..128,
/// `^[a-zA-Z0-9-]+$` (from the model's `DeploymentId` member constraints), so the rows
/// span both published length bounds and the forms the pattern allows.
fn unknown_deployment_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the recorded id", "sim2real0000000000nosuch".to_string()),
        ("minimum length 2", "ab".to_string()),
        ("maximum length 128", "d".repeat(128)),
        ("hyphens and mixed case", "Nope-NOT-Here-9".to_string()),
        // A `deploymentId` is minted as a hyphenated UUID, so a well-formed id nobody
        // minted is one of those too.
        (
            "shaped like a minted id",
            "3f8e1c2d-5a6b-47c8-9d0e-1f2a3b4c5d6e".to_string(),
        ),
        ("hyphenated and sequential", "lwd-99999999".to_string()),
        ("all digits", "1234567890".to_string()),
    ]
}

/// Well-formed `workloadName`s the catalogue does not hold: `WorkloadName` is len
/// 1..100, `^[A-Za-z][a-zA-Z0-9-_]*$`, so the rows span both published length bounds,
/// both extra characters the pattern allows, and a case variant of a seeded name
/// (lookup is by exact name).
fn unknown_workload_names() -> Vec<(&'static str, String)> {
    vec![
        ("the recorded name", "Sim2RealNoSuchWorkloadXyz".to_string()),
        ("minimum length 1", "Z".to_string()),
        (
            "maximum length 100",
            format!("W{}", "o".repeat(99)),
        ),
        ("underscore and hyphen", "No_Such-Workload9".to_string()),
        ("lowercase variant of a seeded name", "sap".to_string()),
    ]
}

/// Every operation that resolves a `deploymentId`, with the request body that carries
/// only that id.
fn deployment_lookup_paths() -> [&'static str; 3] {
    ["/getDeployment", "/deleteDeployment", "/listDeploymentEvents"]
}

#[tokio::test]
async fn a_deployment_lookup_that_finds_nothing_carries_the_fixed_message() {
    let harness = Harness::new();
    // A stored deployment exists throughout, so a 404 below is about the id looked up
    // rather than about an empty store.
    let stored = create(&harness, "seed", "SAP", "SapHanaSingle", "fixed-message-seed").await;

    for path in deployment_lookup_paths() {
        for (label, id) in unknown_deployment_ids() {
            let case = format!("{path} {label}");
            let body = harness
                .refused(
                    &case,
                    path,
                    json!({"deploymentId": id}),
                    404,
                    "ResourceNotFoundException",
                )
                .await;
            assert_fixed_message(&case, &body, DEPLOYMENT_NOT_FOUND, &id);
        }
    }

    // The same operations still resolve the id that does exist, so the fixed message is
    // the answer to "not found" and not to every request.
    let found = harness
        .ok(
            "stored deployment",
            "/getDeployment",
            json!({"deploymentId": stored}),
        )
        .await;
    assert_eq!(
        found["deployment"]["id"],
        json!(stored),
        "the stored deployment must still be readable, got {found}"
    );
}

#[tokio::test]
async fn deployment_ids_outside_the_published_bounds_are_validation_errors() {
    let harness = Harness::new();
    // One past each published bound of `DeploymentId` (len 2..128), and one form the
    // pattern refuses: a constraint failure is a 400, never the not-found answer.
    let rejected = [
        ("one below the minimum length", "a".to_string()),
        ("one above the maximum length", "d".repeat(129)),
        ("character outside the pattern", "no such id".to_string()),
    ];
    for path in deployment_lookup_paths() {
        for (label, id) in &rejected {
            let case = format!("{path} {label}");
            harness
                .refused(
                    &case,
                    path,
                    json!({"deploymentId": id}),
                    400,
                    "ValidationException",
                )
                .await;
        }
    }
}

#[tokio::test]
async fn a_workload_lookup_that_finds_nothing_carries_the_fixed_message() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;
    let known_pattern = first_pattern(&names[0]);

    for (label, workload) in unknown_workload_names() {
        // GetWorkload: the direct lookup — a 404 with the fixed message, not a 501.
        let case = format!("GetWorkload {label}");
        let body = harness
            .refused(
                &case,
                "/getWorkload",
                json!({"workloadName": workload}),
                404,
                "ResourceNotFoundException",
            )
            .await;
        assert_fixed_message(&case, &body, WORKLOAD_NOT_FOUND, &workload);

        // CreateDeployment resolves the same catalogue, so it answers the same way.
        let case = format!("CreateDeployment {label}");
        let body = harness
            .refused(
                &case,
                "/createDeployment",
                create_body(&workload, known_pattern, &format!("unknown-{}", workload.len()), false),
                404,
                "ResourceNotFoundException",
            )
            .await;
        assert_fixed_message(&case, &body, WORKLOAD_NOT_FOUND, &workload);
    }

    // None of those refusals created anything.
    let listed = harness
        .ok("after refusals", "/listDeployments", json!({}))
        .await;
    assert_eq!(
        listed["deployments"].as_array().map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {listed}"
    );
}

#[tokio::test]
async fn workload_names_outside_the_published_bounds_are_validation_errors() {
    let harness = Harness::new();
    // One past the published maximum of `WorkloadName` (len 1..100), the empty name
    // one below its minimum, and two forms `^[A-Za-z][a-zA-Z0-9-_]*$` refuses.
    let rejected = [
        ("one above the maximum length", format!("W{}", "o".repeat(100))),
        ("one below the minimum length", String::new()),
        ("leading digit", "9NotAName".to_string()),
        ("character outside the pattern", "Not A Name".to_string()),
    ];
    for (label, workload) in &rejected {
        harness
            .refused(
                label,
                "/getWorkload",
                json!({"workloadName": workload}),
                400,
                "ValidationException",
            )
            .await;
    }
}

#[tokio::test]
async fn an_unknown_deployment_pattern_carries_one_fixed_identifier_free_message() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;
    let other = names
        .iter()
        .find(|name| *name != &names[0])
        .expect("the catalogue holds more than one workload");
    // `DeploymentPatternName` is len 1..256, `^[A-Za-z0-9][a-zA-Z0-9-]*$`: rows span
    // both published length bounds, a digit-led name, and a pattern that exists but
    // belongs to another workload.
    let unknown = [
        ("plain unknown", "NoSuchPattern".to_string()),
        ("minimum length 1", "Q".to_string()),
        ("maximum length 256", format!("P{}", "a".repeat(255))),
        ("digit-led", "9NoSuchPattern".to_string()),
        ("pattern of another workload", first_pattern(other).to_string()),
    ];

    let mut messages: Vec<String> = Vec::new();
    for (index, (label, pattern)) in unknown.into_iter().enumerate() {
        let case = format!("{}/{label}", names[0]);
        let body = harness
            .refused(
                &case,
                "/createDeployment",
                create_body(&names[0], &pattern, &format!("pattern-{index}"), false),
                404,
                "ResourceNotFoundException",
            )
            .await;
        let actual = error_message(&body);
        assert!(
            !actual.contains(&pattern) && !actual.contains(&names[0]),
            "case {case:?}: the message must render neither the requested pattern nor \
             the workload, got {actual:?}"
        );
        messages.push(actual);
    }
    let first = messages[0].clone();
    for (index, actual) in messages.iter().enumerate() {
        assert_eq!(
            actual, &first,
            "case {index}: every unknown-pattern refusal must carry the same fixed \
             message {first:?}, got {actual:?}"
        );
    }
    assert!(
        !first.is_empty(),
        "an unknown-pattern refusal must carry a message, got {first:?}"
    );
}

#[tokio::test]
async fn get_workload_reports_every_catalogue_entry_from_state() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;
    assert!(
        names.len() >= 2,
        "the catalogue must be seeded with more than one workload, got {names:?}"
    );

    let listed = harness.ok("summaries", "/listWorkloads", json!({})).await;
    let summaries = listed["workloads"]
        .as_array()
        .expect("ListWorkloads must report a workloads list")
        .clone();

    // Every entry the catalogue holds — not only the one the issue cited — is readable
    // through GetWorkload, and reports the same values ListWorkloads does for it.
    for name in &names {
        let body = harness
            .ok(name, "/getWorkload", json!({"workloadName": name}))
            .await;
        let workload = body["workload"].clone();
        assert_eq!(
            workload["workloadName"],
            json!(name),
            "case {name:?}: GetWorkload must report the stored workloadName, got {workload}"
        );
        let status = workload["status"].as_str().unwrap_or_default();
        assert!(
            WORKLOAD_STATUS_VALUES.contains(&status),
            "case {name:?}: status {status:?} is not a WorkloadStatus value"
        );
        let summary = summaries
            .iter()
            .find(|summary| summary["workloadName"] == json!(name))
            .unwrap_or_else(|| panic!("case {name:?}: ListWorkloads must report this workload"));
        for member in ["displayName", "status"] {
            assert_eq!(
                workload[member], summary[member],
                "case {name:?}: GetWorkload's {member} must match ListWorkloads', got {} vs {}",
                workload[member], summary[member]
            );
        }
        assert!(
            workload["displayName"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "case {name:?}: GetWorkload must report a displayName, got {workload}"
        );

        // A read changes nothing a later caller sees: the same call answers the same
        // thing, and the catalogue is unchanged.
        let again = harness
            .ok(name, "/getWorkload", json!({"workloadName": name}))
            .await;
        assert_eq!(
            again, body,
            "case {name:?}: GetWorkload must be a pure read, got {again} then {body}"
        );
    }

    let after = harness.ok("after reads", "/listWorkloads", json!({})).await;
    assert_eq!(
        after, listed,
        "reading workloads must not change the catalogue, got {after} then {listed}"
    );
}
