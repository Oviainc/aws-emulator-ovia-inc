// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeploymentData$resourceGroup`: the group holding a
//! deployment's own resources.
//!
//! The rule under test: a deployment reports the group holding its resources from the
//! moment it reaches a status in which those resources are being or have been
//! provisioned, as a name derived from the deployment's own name; while provisioning
//! has not started — a deployment still being created or still having its settings
//! checked, and one deleted before provisioning ever started — no group exists, so the
//! member is absent, and once reported the name never changes.
//!
//! Asserted over a TABLE rather than over one call: the rows differ in the deployment
//! NAME the group is named after (each character class `DeploymentName` allows, its
//! published length bounds, one row per seeded workload and deployment pattern) and in
//! how far the deployment has got (every status a create walks, mid-update, after an
//! update finished, through a delete, and deleted before provisioning started).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::delete_deployment_status_sequence::one_pattern_per_workload;
use crate::deployment_lifecycle::{create, get};
use crate::harness::Harness;

/// The name a deployment's resource group carries is the deployment's own name behind
/// this prefix. Written out here rather than read from the crate, so the suite pins
/// the value that goes on the wire independently of the constant the service builds it
/// from.
const EXPECTED_PREFIX: &str = "LaunchWizard-";

/// The `DeploymentStatus` members a deployment is in before any of its resources
/// exist: it is still being created, or its settings are still being checked. Both are
/// members of the model's own enum (smithy_query `shape="DeploymentStatus"`).
const PRE_PROVISIONING: [&str; 2] = ["CREATING", "VALIDATING"];

/// `DeploymentName` is len 1..50 over `^[A-Za-z0-9_\.-]+$` (smithy_query:
/// `shape="DeploymentName"`), so 50 is its published ceiling. Its floor is not a length
/// this suite can use: `CreateDeployment` judges a name by a stricter rule of its own
/// (`state::DEPLOYMENT_NAME_SERVER_PATTERN`, driven by
/// `tests/all/create_deployment_finding_report.rs`), under which the shortest name a
/// deployment can carry is two characters.
const NAME_MIN_LEN: usize = 2;
/// See [`NAME_MIN_LEN`].
const NAME_MAX_LEN: usize = 50;

/// How long a poll waits for a deployment to leave a non-terminal status, and how
/// often it re-reads. The emulator never sleeps; this is the test waiting for the
/// clock-owed step.
const DEADLINE: Duration = Duration::from_millis(6_000);
/// See [`DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `resourceGroup` of the deployment `id`, and the `status` it was read at.
async fn group_and_status(harness: &Harness, case: &str, id: &str) -> (Option<String>, String) {
    let deployment = get(harness, case, id).await;
    let status = deployment["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetDeployment reported no status: {deployment}"))
        .to_string();
    let group = deployment
        .get("resourceGroup")
        .and_then(Value::as_str)
        .map(str::to_string);
    (group, status)
}

/// Poll `GetDeployment` until the deployment's status satisfies `done`, returning
/// every `(resourceGroup, status)` pair observed on the way.
async fn poll_until(
    harness: &Harness,
    case: &str,
    id: &str,
    done: impl Fn(&str) -> bool,
) -> Vec<(Option<String>, String)> {
    let started = Instant::now();
    let mut observed: Vec<(Option<String>, String)> = Vec::new();
    loop {
        let reading = group_and_status(harness, case, id).await;
        if observed.last() != Some(&reading) {
            observed.push(reading.clone());
        }
        if done(&reading.1) {
            return observed;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {case:?}: the deployment never reached the awaited status within \
             {DEADLINE:?}, observed {observed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A resource group name is within the rules a group name has: non-empty, no longer
/// than 128 characters, and made only of letters, digits, hyphens, underscores and
/// periods — the character set `DeploymentName` itself is drawn from.
fn assert_well_formed(case: &str, group: &str) {
    assert!(
        (1..=128).contains(&group.len()),
        "case {case:?}: a resource group name is 1..128 characters, got {} in {group:?}",
        group.len()
    );
    assert!(
        group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "case {case:?}: a resource group name holds only letters, digits, '-', '_' and \
         '.', got {group:?}"
    );
}

/// The deployment names this suite spans: every character class a name a deployment can
/// be created under may hold, and both bounds of the length it may have, each one a
/// different length so the reported group name cannot be a fixed string.
fn deployment_names() -> Vec<(&'static str, String)> {
    vec![
        // The shortest name a deployment can carry.
        ("shortest name", "a".repeat(NAME_MIN_LEN)),
        // Every character class the rule allows, mixed.
        ("all character classes", "Rg1name2".to_string()),
        // Digits after the leading letter, so the name is not word-shaped.
        ("digits after the leading letter", "n0123456789".to_string()),
        // A leading capital, one character past the shortest name.
        ("a leading capital", "Rg1".to_string()),
        // The longest name a deployment can carry.
        ("longest name", "L".repeat(NAME_MAX_LEN)),
    ]
}

#[tokio::test]
async fn the_reported_group_is_named_after_the_deployment_once_it_is_past_creation() {
    let harness = Harness::new();
    let patterns = one_pattern_per_workload(&harness).await;
    let mut reported: Vec<String> = Vec::new();

    for (row, (label, name)) in deployment_names().into_iter().enumerate() {
        let (workload, pattern) = patterns[row % patterns.len()].clone();
        let case = format!("{label} ({workload}/{pattern})");
        let id = create(&harness, &case, &workload, &pattern, &name).await;

        // While the deployment is still being created nothing has been provisioned, so
        // there is no group to report. The create's own step is owed by the clock and
        // its span is non-zero, so this reading is of a deployment still in CREATING.
        let (group, status) = group_and_status(&harness, &case, &id).await;
        assert_eq!(
            status, "CREATING",
            "case {case:?}: a deployment read straight after its create is still being \
             created, got {status:?}"
        );
        assert_eq!(
            group, None,
            "case {case:?}: a deployment at {status:?} has provisioned nothing, so it \
             reports no resource group, got {group:?}"
        );

        // Once the clock starts provisioning its resources the group exists and is
        // named after the deployment — at EVERY status from there onwards, and at none
        // of the ones before it.
        let group = format!("{EXPECTED_PREFIX}{name}");
        let observed = poll_until(&harness, &case, &id, |status| status == "COMPLETED").await;
        for (seen, status) in &observed {
            let expected = (!PRE_PROVISIONING.contains(&status.as_str()))
                .then_some(group.as_str());
            assert_eq!(
                seen.as_deref(),
                expected,
                "case {case:?}: at {status:?} the deployment must report {expected:?} as its \
                 resource group, got {seen:?} (readings: {observed:?})"
            );
        }
        assert!(
            observed
                .iter()
                .any(|(seen, _)| seen.as_deref() == Some(group.as_str())),
            "case {case:?}: a deployment whose resources were provisioned must report \
             {group:?}, observed {observed:?}"
        );
        assert_well_formed(&case, &group);

        // It is the SAME value on a second read: a read computes it from the record
        // rather than minting it.
        let (again, _) = group_and_status(&harness, &case, &id).await;
        assert_eq!(
            again.as_deref(),
            Some(group.as_str()),
            "case {case:?}: the reported group is stable across reads, expected {group:?}, \
             got {again:?}"
        );

        // An update neither re-names nor drops it, at either of the statuses an update
        // moves the deployment through.
        harness
            .ok(
                &case,
                "/updateDeployment",
                json!({
                    "deploymentId": id,
                    "specifications": {"KeyPairName": "key-for-resource-group"},
                    "force": true,
                }),
            )
            .await;
        let updating = poll_until(&harness, &case, &id, |status| {
            status == "UPDATE_COMPLETED"
        })
        .await;
        for (seen, status) in &updating {
            assert_eq!(
                seen.as_deref(),
                Some(group.as_str()),
                "case {case:?}: at {status:?} the deployment still owns {group:?}, got {seen:?}"
            );
        }

        // And a delete of a deployment whose resources exist does not rewrite the group
        // the record owned either.
        harness
            .ok(&case, "/deleteDeployment", json!({"deploymentId": id}))
            .await;
        let deleting = poll_until(&harness, &case, &id, |status| status == "DELETED").await;
        for (seen, status) in &deleting {
            assert_eq!(
                seen.as_deref(),
                Some(group.as_str()),
                "case {case:?}: at {status:?} the record still names the group it owned, \
                 expected {group:?}, got {seen:?}"
            );
        }

        reported.push(group);
    }

    // Each deployment owns its own group: the names are distinct because they are
    // derived from the deployment, not from a constant.
    let mut distinct = reported.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported.len(),
        "two deployments must not report the same resource group, got {reported:?}"
    );
}

#[tokio::test]
async fn a_deployment_deleted_before_it_is_provisioned_never_reports_a_group() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");

    // The rows differ in the name the group WOULD have been named after, so a group
    // reported here could not be mistaken for a fixed value.
    for (label, name) in deployment_names() {
        let case = format!("deleted before provisioning: {label} ({workload}/{pattern})");
        let id = create(&harness, &case, &workload, &pattern, &name).await;

        // Deleting straight away: the deployment never reaches a status in which its
        // resources exist, so no group is ever created for it.
        let answered = harness
            .ok(&case, "/deleteDeployment", json!({"deploymentId": id}))
            .await;
        assert_eq!(
            answered["status"].as_str(),
            Some("DELETE_INITIATING"),
            "case {case:?}: the delete must report the status it just entered, got {answered}"
        );
        let observed = poll_until(&harness, &case, &id, |status| status == "DELETED").await;
        for (group, status) in &observed {
            assert_eq!(
                group.as_deref(),
                None,
                "case {case:?}: at {status:?} the deployment provisioned nothing, so it \
                 reports no resource group, got {group:?}"
            );
        }
    }
}
