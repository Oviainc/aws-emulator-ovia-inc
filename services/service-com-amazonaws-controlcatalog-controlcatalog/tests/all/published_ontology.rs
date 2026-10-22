// SPDX-License-Identifier: CC0-1.0

//! The Control Catalog serves the ontology it PUBLISHES, not a sample of it.
//!
//! `ListCommonControls` is documented as returning "a paginated list of common controls
//! from the Amazon Web Services Control Catalog", with a filter that "narrows the
//! results to a specific objective" and no filter returning "all common controls"
//! (`smithy_query(query="doc", shape="com.amazonaws.controlcatalog#ListCommonControls")`).
//! So the rule under test here is a COVERAGE rule rather than a per-entry one: every
//! domain the catalog publishes carries objectives, every one of those objectives
//! carries the common controls published under it, and a client that walks domain ->
//! objective -> common control reaches them wherever in the ontology it starts.
//!
//! Each test is a TABLE: either the hand-written rows below, which span the published
//! domains, or rows generated from the catalog the service itself serves.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde_json::Value;

use crate::control_catalog::{at_path, by_arn, drain, Emulator};

/// Rows of the ontology the Control Catalog publishes — `domain -> objective ->
/// common control` — one per published domain.
///
/// Source: the common-control library Amazon Web Services publishes for the Control
/// Catalog, as this emulator serves it in
/// `src/state/published_ontology.rs` (that module records the provenance of the table
/// and of the ARN identifiers). The rows deliberately reach into the corners of the
/// ontology rather than its first entries: domains such as data privacy, governance and
/// risk management, third-party management and workforce management are exactly the
/// slices a sampled catalog leaves empty.
///
/// `(domain name, objective name, common control name)`.
const PUBLISHED_ROWS: [(&str, &str, &str); 14] = [
    (
        "Asset management",
        "Asset handling",
        "Removable media handling",
    ),
    (
        "Business continuity and recovery",
        "Backup and recovery",
        "Backup restoration testing",
    ),
    (
        "Data protection",
        "Data loss prevention",
        "Sensitive data discovery",
    ),
    (
        "Identity and access management",
        "Authentication",
        "Session timeout enforcement",
    ),
    (
        "Logging and monitoring",
        "Monitoring and alerting",
        "Clock synchronization",
    ),
    (
        "Network security",
        "Network traffic control",
        "Outbound traffic restriction",
    ),
    (
        "Physical security",
        "Environmental protection",
        "Fire detection and suppression",
    ),
    (
        "Secure configuration management",
        "Endpoint protection",
        "Application allowlisting",
    ),
    (
        "Secure development lifecycle and change management",
        "Release management",
        "Artifact integrity verification",
    ),
    (
        "Incident response",
        "Incident handling",
        "Breach notification",
    ),
    (
        "Third-party management",
        "Supplier monitoring",
        "Supplier access termination",
    ),
    (
        "Workforce management",
        "Security awareness and training",
        "Phishing simulation",
    ),
    (
        "Governance and risk management",
        "Policy management",
        "Policy exception approval",
    ),
    (
        "Data privacy",
        "Data subject rights",
        "Data subject request handling",
    ),
];

/// The maximum page size `ListCommonControls` admits: the top of the modeled
/// `MaxListCommonControlsResults` range (1..100).
const MAX_LIST_COMMON_CONTROLS_RESULTS: u32 = 100;

/// The `Name` of every item in `items`.
fn names(items: &[Value]) -> BTreeSet<&str> {
    items
        .iter()
        .filter_map(|item| at_path(item, &["Name"]))
        .collect()
}

/// Find the item with this `Name`, or panic naming the case.
fn with_name<'a>(items: &'a [Value], name: &str, case: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["Name"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an entry named {name:?}, got {:?}",
                names(items)
            )
        })
}

/// A member of an item, as a string, or a panic naming the case.
fn member<'a>(item: &'a Value, path: &[&str], case: &str) -> &'a str {
    at_path(item, path).unwrap_or_else(|| {
        panic!("case {case}: {path:?} is missing or not a string in {item:?}")
    })
}

/// Every row of the published ontology is reachable by the walk a client makes:
/// `ListDomains` -> `ListObjectives` filtered to that domain -> `ListCommonControls`
/// filtered to that objective.
#[tokio::test]
async fn every_published_domain_serves_its_objectives_and_their_common_controls() {
    let emulator = Emulator::new();
    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;

    for (domain_name, objective_name, common_control_name) in PUBLISHED_ROWS {
        let case = format!("{domain_name} / {objective_name} / {common_control_name}");

        let domain = with_name(&domains, domain_name, &case);
        let domain_arn = member(domain, &["Arn"], &case);

        // Page size 1, so the filter and the pagination cursor are exercised together.
        let objectives = drain(
            &emulator,
            "/objectives",
            "Objectives",
            Some(1),
            &format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{domain_arn}"}}]}}}}"#),
        )
        .await;
        let objective = with_name(&objectives, objective_name, &case);
        let objective_arn = member(objective, &["Arn"], &case);

        let common_controls = drain(
            &emulator,
            "/common-controls",
            "CommonControls",
            Some(2),
            &format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{objective_arn}"}}]}}}}"#),
        )
        .await;
        let common_control = with_name(&common_controls, common_control_name, &case);

        // Every member `CommonControlSummary` declares, resolved out of the same
        // catalog ListDomains and ListObjectives report.
        assert_eq!(
            member(common_control, &["Objective", "Arn"], &case),
            objective_arn,
            "case {case}: the common control must report the objective it was filtered by"
        );
        assert_eq!(
            member(common_control, &["Objective", "Name"], &case),
            objective_name,
            "case {case}: the common control must report the objective's published name"
        );
        assert_eq!(
            member(common_control, &["Domain", "Arn"], &case),
            domain_arn,
            "case {case}: the common control must report its objective's domain"
        );
        assert_eq!(
            member(common_control, &["Domain", "Name"], &case),
            domain_name,
            "case {case}: the common control must report the domain's published name"
        );
        let arn = member(common_control, &["Arn"], &case);
        assert!(
            arn.starts_with("arn:aws:controlcatalog:::common-control/"),
            "case {case}: expected a common-control ARN, got {arn:?}"
        );
        assert!(
            !member(common_control, &["Description"], &case).is_empty(),
            "case {case}: a published common control carries a description"
        );
        let create_time = common_control["CreateTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: CreateTime is missing from {common_control:?}")
        });
        let last_update_time = common_control["LastUpdateTime"]
            .as_f64()
            .unwrap_or_else(|| {
                panic!("case {case}: LastUpdateTime is missing from {common_control:?}")
            });
        assert!(
            last_update_time >= create_time,
            "case {case}: LastUpdateTime {last_update_time} precedes CreateTime {create_time}"
        );
    }
}

/// No published domain or objective is an empty slice of the catalog: every domain has
/// objectives, every objective has common controls, and the per-objective filters
/// partition the whole common-control population.
///
/// The rows here are generated from the catalog the service serves, so the rule is
/// checked over every entry rather than the fourteen named above.
#[tokio::test]
async fn no_published_domain_or_objective_answers_with_an_empty_slice() {
    let emulator = Emulator::new();
    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;
    let all_objectives = drain(&emulator, "/objectives", "Objectives", None, "{}").await;
    let all_common_controls =
        drain(&emulator, "/common-controls", "CommonControls", None, "{}").await;

    // Uniqueness over each collection's names and ARNs: a slug duplicated in the
    // published table would collide in the ARN that keys the catalog and drop an entry,
    // which shows up here as a count that does not match.
    for (collection, items) in [
        ("domains", &domains),
        ("objectives", &all_objectives),
        ("common controls", &all_common_controls),
    ] {
        assert_eq!(
            names(items).len(),
            items.len(),
            "case {collection}: {} entries carry only {} distinct names",
            items.len(),
            names(items).len()
        );
        let arns: BTreeSet<&str> = items
            .iter()
            .filter_map(|item| at_path(item, &["Arn"]))
            .collect();
        assert_eq!(
            arns.len(),
            items.len(),
            "case {collection}: {} entries carry only {} distinct ARNs",
            items.len(),
            arns.len()
        );
    }

    let mut objectives_seen = 0usize;
    let mut union: Vec<Value> = Vec::new();
    // How many common controls each objective carries: the ontology publishes a
    // different number under each, so this also pins that the filter counts per
    // objective rather than answering with a fixed slice.
    let mut per_objective: BTreeMap<String, usize> = BTreeMap::new();

    for domain in &domains {
        let domain_name = member(domain, &["Name"], "a domain summary");
        let domain_arn = member(domain, &["Arn"], domain_name);
        let objectives = drain(
            &emulator,
            "/objectives",
            "Objectives",
            None,
            &format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{domain_arn}"}}]}}}}"#),
        )
        .await;
        assert!(
            !objectives.is_empty(),
            "case domain {domain_name}: a published domain carries objectives"
        );
        objectives_seen += objectives.len();

        for objective in &objectives {
            let objective_name = member(objective, &["Name"], domain_name);
            let objective_arn = member(objective, &["Arn"], objective_name);
            let common_controls = drain(
                &emulator,
                "/common-controls",
                "CommonControls",
                None,
                &format!(
                    r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{objective_arn}"}}]}}}}"#
                ),
            )
            .await;
            assert!(
                !common_controls.is_empty(),
                "case objective {objective_name} (domain {domain_name}): a published \
                 objective carries the common controls that meet it"
            );
            for common_control in &common_controls {
                let case = format!("{objective_name} / {}", common_control["Arn"]);
                // The members `CommonControlSummary` declares, over every entry.
                for path in [
                    vec!["Arn"],
                    vec!["Name"],
                    vec!["Description"],
                    vec!["Domain", "Arn"],
                    vec!["Domain", "Name"],
                    vec!["Objective", "Arn"],
                    vec!["Objective", "Name"],
                ] {
                    assert!(
                        !member(common_control, &path, &case).is_empty(),
                        "case {case}: {path:?} came back empty"
                    );
                }
                assert_eq!(
                    member(common_control, &["Objective", "Arn"], &case),
                    objective_arn,
                    "case {case}: the filter returned a common control of another objective"
                );
                assert_eq!(
                    member(common_control, &["Domain", "Arn"], &case),
                    domain_arn,
                    "case {case}: the common control reports a domain that is not its \
                     objective's"
                );
            }
            per_objective.insert(objective_arn.to_string(), common_controls.len());
            union.extend(common_controls);
        }
    }

    assert_eq!(
        objectives_seen,
        all_objectives.len(),
        "the per-domain filters must partition every objective the catalog publishes"
    );
    assert_eq!(
        by_arn(union),
        by_arn(all_common_controls.clone()),
        "the per-objective filters must partition every common control the catalog \
         publishes"
    );
    let distinct_sizes: BTreeSet<usize> = per_objective.values().copied().collect();
    assert!(
        distinct_sizes.len() > 1,
        "the published objectives carry different numbers of common controls, so the \
         filtered counts must differ: {per_objective:?}"
    );
}

/// The published common-control population is larger than one maximum-sized page, so a
/// paginated walk does not terminate on its first page — and walking it with the
/// maximum page size visits every entry exactly once, in the same order as the
/// smallest.
#[tokio::test]
async fn the_common_control_population_outgrows_one_maximum_sized_page() {
    let emulator = Emulator::new();

    let first_page = emulator
        .ok(
            &format!("/common-controls?maxResults={MAX_LIST_COMMON_CONTROLS_RESULTS}"),
            "{}",
        )
        .await;
    let returned = first_page["CommonControls"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        returned, MAX_LIST_COMMON_CONTROLS_RESULTS as usize,
        "case maxResults={MAX_LIST_COMMON_CONTROLS_RESULTS}: the published catalog fills a \
         maximum-sized page, got {returned} entries"
    );
    assert!(
        first_page
            .get("NextToken")
            .and_then(Value::as_str)
            .is_some(),
        "case maxResults={MAX_LIST_COMMON_CONTROLS_RESULTS}: entries remain after a full \
         page, so a usable NextToken must come back: {first_page:?}"
    );

    // Each page size spans the whole population, in one order.
    let whole = drain(&emulator, "/common-controls", "CommonControls", None, "{}").await;
    for page_size in [1u32, 7, MAX_LIST_COMMON_CONTROLS_RESULTS] {
        let paged = drain(
            &emulator,
            "/common-controls",
            "CommonControls",
            Some(page_size),
            "{}",
        )
        .await;
        assert_eq!(
            paged, whole,
            "case maxResults={page_size}: a paginated walk must yield the same common \
             controls, in the same order, as the default page size"
        );
    }
    assert!(
        whole.len() > MAX_LIST_COMMON_CONTROLS_RESULTS as usize,
        "the published catalog holds more common controls than one page: {} entries",
        whole.len()
    );
}
