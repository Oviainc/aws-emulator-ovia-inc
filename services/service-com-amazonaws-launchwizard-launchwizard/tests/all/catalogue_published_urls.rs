// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the URLs the service-owned workload catalogue publishes:
//! `WorkloadData$documentationUrl` and `WorkloadData$iconUrl`, reported by
//! `GetWorkload`, driven over HTTP through the same router the production emulator
//! serves with.
//!
//! The rule under test is not about one entry: a reporting operation answers every
//! member of its output shape that the catalogue entry it projects holds a value for,
//! and a catalogue entry holds the document and the icon the service publishes for it.
//! So each of those members is reported for EVERY entry, as an absolute URL that is the
//! entry's own and that reading does not change — and a member the entry holds NO value
//! for is left out rather than filled from another record, which is why a deployment
//! pattern VERSION publishes no document of its own: the catalogue publishes one
//! deployment document per application, not one per pattern version.
//!
//! Every assertion therefore runs over a TABLE: every workload the catalogue reports
//! (not only the one entry the report cited), every deployment pattern of each, and
//! every version each pattern publishes.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::CATALOGUE_PATTERN_VERSIONS;

use crate::deployment_update::every_pattern;
use crate::harness::{catalogue, Harness};

/// The `WorkloadData` entry `GetWorkload` reports for `name`.
async fn get_workload(harness: &Harness, case: &str, name: &str) -> Value {
    let body = harness
        .ok(case, "/getWorkload", json!({"workloadName": name}))
        .await;
    let workload = body["workload"].clone();
    assert!(
        workload.is_object(),
        "case {case:?}: GetWorkload must report a workload, got {body}"
    );
    workload
}

/// The `DeploymentPatternVersionDataSummary` `GetDeploymentPatternVersion` reports.
async fn get_version(
    harness: &Harness,
    case: &str,
    workload: &str,
    pattern: &str,
    version: &str,
) -> Value {
    let body = harness
        .ok(
            case,
            "/getDeploymentPatternVersion",
            json!({
                "workloadName": workload,
                "deploymentPatternName": pattern,
                "deploymentPatternVersionName": version,
            }),
        )
        .await;
    let summary = body["deploymentPatternVersion"].clone();
    assert!(
        summary.is_object(),
        "case {case:?}: GetDeploymentPatternVersion must report a version, got {body}"
    );
    summary
}

/// The value of a URL member, required to be a non-empty string.
fn url_member(reported: &Value, case: &str, member: &str) -> String {
    let value = reported[member].as_str().unwrap_or_default().to_string();
    assert!(
        !value.is_empty(),
        "case {case:?}: every catalogue entry must report a non-empty {member}, got {}",
        reported[member]
    );
    value
}

/// True when `value` is an absolute `https` URL: the scheme, a non-empty host, and no
/// whitespace anywhere.
///
/// The model types every one of these members as a plain `String` with no `@pattern`
/// (`smithy_query(query="shape", shape="WorkloadData,DeploymentPatternVersionDataSummary")`),
/// so what makes a reported value usable is that it is a URL a client could fetch
/// rather than a shape the model pins.
fn absolute_https_url(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && !value.contains(char::is_whitespace)
}

#[tokio::test]
async fn every_catalogue_entry_reports_its_own_document_and_icon_url() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;
    assert!(
        names.len() >= 2,
        "the catalogue must hold more than one workload for this table to span it, got {names:?}"
    );

    // One row per catalogue entry, and one column per URL member `WorkloadData`
    // declares, so the rule is asserted over the whole catalogue rather than one entry.
    let mut documents: BTreeSet<String> = BTreeSet::new();
    let mut icons: BTreeSet<String> = BTreeSet::new();
    for name in &names {
        let workload = get_workload(&harness, name, name).await;
        for member in ["documentationUrl", "iconUrl"] {
            let url = url_member(&workload, name, member);
            assert!(
                absolute_https_url(&url),
                "case {name:?}: {member} must be an absolute https URL, got {url:?}"
            );
        }
        documents.insert(url_member(&workload, name, "documentationUrl"));
        icons.insert(url_member(&workload, name, "iconUrl"));

        // Reading an entry changes nothing a later caller sees: the same request
        // answers the same record, URLs included.
        let again = get_workload(&harness, name, name).await;
        assert_eq!(
            again, workload,
            "case {name:?}: reading a workload must not change what a later caller sees"
        );
    }

    // Each entry publishes ITS OWN document and icon, so the catalogue tells its
    // entries apart rather than answering every one with a single canned URL.
    assert_eq!(
        documents.len(),
        names.len(),
        "every catalogue entry must report a documentationUrl of its own, got {documents:?}"
    );
    assert_eq!(
        icons.len(),
        names.len(),
        "every catalogue entry must report an iconUrl of its own, got {icons:?}"
    );
}

#[tokio::test]
async fn the_listing_reports_no_url_member_its_summary_shape_does_not_declare() {
    let harness = Harness::new();
    // `WorkloadDataSummary` declares only workloadName/displayName/status/
    // accountConstraints (`smithy_query(query="shape", shape="WorkloadDataSummary")`):
    // the two URLs are members `WorkloadData` ADDS, so the listing must not grow them
    // just because the detail view reports them.
    let body = harness.ok("listWorkloads", "/listWorkloads", json!({})).await;
    let summaries = body["workloads"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWorkloads must report a workloads list, got {body}"))
        .clone();
    assert!(
        !summaries.is_empty(),
        "the catalogue must report at least one workload, got {body}"
    );
    for summary in &summaries {
        let name = summary["workloadName"].as_str().unwrap_or_default();
        for member in ["documentationUrl", "iconUrl"] {
            assert!(
                summary.get(member).is_none_or(Value::is_null),
                "case {name:?}: WorkloadDataSummary declares no {member}, got {}",
                summary[member]
            );
        }
    }
}

#[tokio::test]
async fn no_published_version_reports_a_document_of_its_own() {
    let harness = Harness::new();
    assert!(
        CATALOGUE_PATTERN_VERSIONS.len() >= 2,
        "the catalogue must publish enough versions per pattern to span the rule, got {:?}",
        CATALOGUE_PATTERN_VERSIONS
    );
    // One row per (workload, pattern, version) the catalogue holds: the rule is about
    // the member being read off the stored record, so it holds for every version of
    // every pattern of every workload and not only the newest of one.
    for (workload, pattern) in every_pattern(&harness).await {
        // The document the catalogue DOES publish for the application, which the
        // workload's own view reports.
        let document = url_member(
            &get_workload(&harness, &workload, &workload).await,
            &workload,
            "documentationUrl",
        );
        for version in CATALOGUE_PATTERN_VERSIONS {
            let case = format!("{workload}/{pattern}@{version}");
            let summary = get_version(&harness, &case, &workload, &pattern, version).await;
            // `DeploymentPatternVersionDataSummary$documentationUrl` is the document of
            // THAT VERSION, and the catalogue publishes none per version, so the member
            // is absent rather than filled with the application's document.
            assert!(
                summary.get("documentationUrl").is_none_or(Value::is_null),
                "case {case:?}: a version publishes no document of its own, so the member must \
                 be left out, got {}",
                summary["documentationUrl"]
            );
            assert!(
                !summary.to_string().contains(&document),
                "case {case:?}: a version must not report the application's document \
                 {document:?} as its own, got {summary}"
            );
        }
    }
}
