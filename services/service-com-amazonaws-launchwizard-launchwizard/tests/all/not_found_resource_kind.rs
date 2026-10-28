// SPDX-License-Identifier: CC0-1.0

//! Cover for WHICH not-found message a refusal carries, driven over HTTP through the
//! same router the production emulator serves with.
//!
//! The rule under test: a lookup that resolves no resource answers
//! `ResourceNotFoundException` with the fixed message of the KIND of resource that is
//! actually missing — the most specific kind the request named that the service does
//! not hold — and never the message of an enclosing resource the same request already
//! resolved. So one kind has ONE message however the caller reached it, and two kinds
//! do not share one.
//!
//! The kinds this model publishes are the ones its operations resolve by name: a
//! deployment (`deploymentId`), a workload (`workloadName`), a deployment pattern
//! (`deploymentPatternName`) and a deployment pattern version
//! (`deploymentPatternVersionName`, its own resource with its own read operations,
//! `GetDeploymentPatternVersion` and `ListDeploymentPatternVersions`). A workload
//! VERSION is not among them: the model binds no operation that reports one, so a
//! `workloadVersionName` the catalogue does not publish is a workload it does not hold
//! at that version — checked below with the workload rows.
//!
//! Every assertion runs over a TABLE: every workload the catalogue holds, each kind,
//! and each operation that resolves that kind, with the absent names spanning the
//! forms and both published length bounds of the shapes that carry them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::CATALOGUE_PATTERN_VERSIONS;

use crate::deployment_lifecycle::{create, get};
use crate::deployment_pattern_versions::version_body;
use crate::deployment_update::{
    conforming_specifications, error_message, one_pattern_per_workload, pattern_data, update_body,
    with_member,
};
use crate::harness::Harness;

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s it walks, plus slack): a deployment whose create is still
/// in flight refuses an update, and the update rows below need one at rest.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// A well-formed `deploymentId` no deployment has (`DeploymentId` is len 2..128,
/// `^[a-zA-Z0-9-]+$`).
const UNKNOWN_DEPLOYMENT: &str = "no-such-deployment-9";

/// One workload's row: the deployment the update refusals run against, the pattern it
/// deploys, and settings that pattern's schema accepts (so a refusal below is about the
/// name that is missing and not about the settings).
struct Row {
    case: String,
    workload: String,
    pattern: String,
    specifications: Map<String, Value>,
    id: String,
}

/// Well-formed `deploymentPatternVersionName`s the catalogue does not publish.
///
/// `DeploymentPatternVersionName` is @length(min: 5, max: 30) with
/// @pattern `^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$` — from
/// smithy_query(query="shape", shape="DeploymentPatternVersionName") — so the rows span
/// both forms it allows and each published length bound, and none of them is an entry
/// of `state::CATALOGUE_PATTERN_VERSIONS`.
fn absent_versions() -> Vec<(&'static str, String)> {
    vec![
        ("a dotted version", "9.9.9".to_string()),
        ("a dashed version", "no-such-version".to_string()),
        ("minimum length 5", "ab-cd".to_string()),
        ("maximum length 30", format!("v{}", "9".repeat(29))),
    ]
}

/// Well-formed `deploymentPatternName`s no workload of the catalogue holds
/// (`DeploymentPatternName` is len 1..256, `^[A-Za-z0-9][a-zA-Z0-9-]*$`).
fn absent_patterns() -> Vec<(&'static str, String)> {
    vec![
        ("plain unknown", "NoSuchPattern".to_string()),
        ("minimum length 1", "Q".to_string()),
        ("maximum length 256", format!("P{}", "a".repeat(255))),
        ("digit-led", "9NoSuchPattern".to_string()),
    ]
}

/// Well-formed `workloadName`s the catalogue does not hold (`WorkloadName` is len
/// 1..100, `^[A-Za-z][a-zA-Z0-9-_]*$`).
fn absent_workloads() -> Vec<(&'static str, String)> {
    vec![
        ("plain unknown", "NoSuchWorkloadXyz".to_string()),
        ("minimum length 1", "Z".to_string()),
        ("maximum length 100", format!("W{}", "o".repeat(99))),
        ("underscore and hyphen", "No_Such-Workload9".to_string()),
    ]
}

/// The message a refused POST of `body` to `path` carries, requiring the modelled
/// 404 `ResourceNotFoundException` and that the message echo none of `probes`.
async fn refusal_message(
    harness: &Harness,
    case: &str,
    path: &str,
    body: Value,
    probes: &[&str],
) -> String {
    let answered = harness
        .refused(case, path, body, 404, "ResourceNotFoundException")
        .await;
    let message = error_message(&answered);
    assert!(
        !message.is_empty(),
        "case {case:?}: a not-found must carry a message, got {answered}"
    );
    for probe in probes {
        // A one- or two-character name can occur in ordinary English by accident, so the
        // echo probe is only meaningful for a longer one.
        assert!(
            probe.len() < 3 || !message.contains(probe),
            "case {case:?}: the message must not render the requested name {probe:?}, got \
             {message:?}"
        );
    }
    message
}

/// Require every message in `collected` (label -> message) to be the SAME string, and
/// hand that string back: one kind of missing resource, one message.
fn one_message_per_kind(kind: &str, collected: &BTreeMap<String, String>) -> String {
    let mut distinct: BTreeSet<&str> = BTreeSet::new();
    for message in collected.values() {
        distinct.insert(message.as_str());
    }
    assert_eq!(
        distinct.len(),
        1,
        "kind {kind:?}: every refusal of this kind must carry ONE message, got {collected:?}"
    );
    collected
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| panic!("kind {kind:?}: no refusal was collected"))
}

#[tokio::test]
async fn a_not_found_carries_the_message_of_the_kind_of_resource_that_is_missing() {
    let harness = Harness::new();
    let published = CATALOGUE_PATTERN_VERSIONS
        .last()
        .copied()
        .expect("the catalogue publishes at least one version per pattern");

    // Every row's deployment is created FIRST and the create lifecycles are waited out
    // with ONE sleep: an update is taken only by a deployment at rest, and a refused
    // update leaves the deployment where it was, so one deployment serves every refusal
    // row of its workload.
    let mut rows: Vec<Row> = Vec::new();
    for (index, (workload, pattern)) in one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .enumerate()
    {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;
        let specifications = conforming_specifications(&case, &data, false);
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("kind-{index}"),
        )
        .await;
        rows.push(Row {
            case,
            workload,
            pattern,
            specifications,
            id,
        });
    }
    std::thread::sleep(CREATE_SETTLE);

    for Row {
        case,
        workload,
        pattern,
        specifications,
        id,
    } in rows
    {
        let stored = get(&harness, &case, &id).await;
        let update = update_body(&id, &specifications);
        let mut per_kind: BTreeMap<&str, String> = BTreeMap::new();

        // KIND: a deployment pattern VERSION. The two operations that resolve a version
        // name resolve it against the same published list, so a version neither holds is
        // one missing resource and carries one message — the VERSION's, not the pattern's,
        // which `UpdateDeployment` has already resolved by the time it reads the version.
        let mut collected: BTreeMap<String, String> = BTreeMap::new();
        for (label, version) in absent_versions() {
            let probes = [version.as_str(), pattern.as_str(), workload.as_str()];
            let read = refusal_message(
                &harness,
                &format!("{case}@{version} GetDeploymentPatternVersion ({label})"),
                "/getDeploymentPatternVersion",
                version_body(&workload, &pattern, &version),
                &probes,
            )
            .await;
            let updated = refusal_message(
                &harness,
                &format!("{case}@{version} UpdateDeployment ({label})"),
                "/updateDeployment",
                with_member(&update, "deploymentPatternVersionName", json!(version)),
                &probes,
            )
            .await;
            assert_eq!(
                updated, read,
                "case {case:?} {label}: an update that names a version the catalogue does \
                 not publish must carry the same missing-version message the version read \
                 carries, got {updated:?} from UpdateDeployment and {read:?} from \
                 GetDeploymentPatternVersion"
            );
            collected.insert(format!("{label} via GetDeploymentPatternVersion"), read);
            collected.insert(format!("{label} via UpdateDeployment"), updated);
        }
        per_kind.insert(
            "deployment pattern version",
            one_message_per_kind("deployment pattern version", &collected),
        );

        // KIND: a deployment PATTERN — every operation that resolves a pattern name of a
        // workload that does exist.
        let mut collected: BTreeMap<String, String> = BTreeMap::new();
        for (label, absent) in absent_patterns() {
            let probes = [absent.as_str(), workload.as_str()];
            for (operation, path, body) in [
                (
                    "GetWorkloadDeploymentPattern",
                    "/getWorkloadDeploymentPattern",
                    json!({"workloadName": workload, "deploymentPatternName": absent}),
                ),
                (
                    "ListDeploymentPatternVersions",
                    "/listDeploymentPatternVersions",
                    json!({"workloadName": workload, "deploymentPatternName": absent}),
                ),
                (
                    "GetDeploymentPatternVersion",
                    "/getDeploymentPatternVersion",
                    version_body(&workload, &absent, published),
                ),
            ] {
                let message = refusal_message(
                    &harness,
                    &format!("{case} {operation} ({label})"),
                    path,
                    body,
                    &probes,
                )
                .await;
                collected.insert(format!("{label} via {operation}"), message);
            }
        }
        per_kind.insert(
            "deployment pattern",
            one_message_per_kind("deployment pattern", &collected),
        );

        // KIND: a WORKLOAD — including the workload VERSION an update may name, which is
        // no resource of its own in this model (no operation reports one), so the
        // catalogue not publishing it is the workload not being held at that version.
        let mut collected: BTreeMap<String, String> = BTreeMap::new();
        for (label, absent) in absent_workloads() {
            let probes = [absent.as_str(), pattern.as_str()];
            for (operation, path, body) in [
                (
                    "GetWorkload",
                    "/getWorkload",
                    json!({"workloadName": absent}),
                ),
                (
                    "ListWorkloadDeploymentPatterns",
                    "/listWorkloadDeploymentPatterns",
                    json!({"workloadName": absent}),
                ),
                (
                    "GetWorkloadDeploymentPattern",
                    "/getWorkloadDeploymentPattern",
                    json!({"workloadName": absent, "deploymentPatternName": pattern}),
                ),
                (
                    "GetDeploymentPatternVersion",
                    "/getDeploymentPatternVersion",
                    version_body(&absent, &pattern, published),
                ),
            ] {
                let message = refusal_message(
                    &harness,
                    &format!("{case} {operation} ({label})"),
                    path,
                    body,
                    &probes,
                )
                .await;
                collected.insert(format!("{label} via {operation}"), message);
            }
        }
        for (label, version) in absent_versions() {
            let message = refusal_message(
                &harness,
                &format!("{case} UpdateDeployment workloadVersionName ({label})"),
                "/updateDeployment",
                with_member(&update, "workloadVersionName", json!(version)),
                &[version.as_str(), pattern.as_str(), workload.as_str()],
            )
            .await;
            collected.insert(format!("{label} via UpdateDeployment"), message);
        }
        per_kind.insert("workload", one_message_per_kind("workload", &collected));

        // KIND: a DEPLOYMENT — resolved before any catalogue name is, so an update that
        // names both an absent deployment and an absent version is refused for the
        // deployment.
        let mut collected: BTreeMap<String, String> = BTreeMap::new();
        let absent_update = update_body(UNKNOWN_DEPLOYMENT, &specifications);
        for (operation, path, body) in [
            (
                "GetDeployment",
                "/getDeployment",
                json!({"deploymentId": UNKNOWN_DEPLOYMENT}),
            ),
            (
                "ListDeploymentEvents",
                "/listDeploymentEvents",
                json!({"deploymentId": UNKNOWN_DEPLOYMENT}),
            ),
            ("UpdateDeployment", "/updateDeployment", absent_update.clone()),
            (
                "UpdateDeployment naming an absent version too",
                "/updateDeployment",
                with_member(&absent_update, "deploymentPatternVersionName", json!("9.9.9")),
            ),
        ] {
            let message = refusal_message(
                &harness,
                &format!("{case} {operation}"),
                path,
                body,
                &[UNKNOWN_DEPLOYMENT],
            )
            .await;
            collected.insert(operation.to_string(), message);
        }
        per_kind.insert("deployment", one_message_per_kind("deployment", &collected));

        // A message identifies the kind that is missing, so two kinds cannot share one:
        // answering the pattern's message for a missing version would collapse these.
        let distinct: BTreeSet<&str> = per_kind.values().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            per_kind.len(),
            "case {case:?}: each kind of missing resource must carry its OWN message, got \
             {per_kind:?}"
        );

        // None of those refusals changed the deployment they were made against.
        let read = get(&harness, &case, &id).await;
        assert_eq!(
            read, stored,
            "case {case:?}: a refused request must persist nothing, got {read} after {stored}"
        );
    }
}
