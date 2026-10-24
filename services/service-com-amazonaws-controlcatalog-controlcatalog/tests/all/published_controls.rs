// SPDX-License-Identifier: CC0-1.0

//! What identifies a published control, and what the catalog publishes one record for.
//!
//! The rules under test, all read off the modeled `ControlSummary` /
//! `GetControlResponse` (`smithy_query(query="operation",
//! operation="ListControls,GetControl")`) and the documentation of their members:
//!
//! 1. The catalog publishes ONE record per published control, each with its own `Arn`,
//!    `Aliases` and `Implementation`. `Implementation` does not identify a control: two
//!    published controls may report the same implementation type and identifier (a
//!    Control Tower detective control and the Config-rule control behind it), so
//!    `ListControls`' `Identifiers` filter answers with EVERY control that carries the
//!    identifier, not one.
//! 2. An alias is a "human-readable designator, such as SH.S3.1" that can "refer to the
//!    same control across different Amazon Web Services services or compliance
//!    frameworks", so it names the control ON the surface that issues it: a `SH.<id>`
//!    designator belongs to the Security Hub control whose own identifier is `<id>`, an
//!    `AWS-GR_…` designator to the control Control Tower publishes under it, and a
//!    `CONFIG.…` designator to a control an AWS Config rule implements. An alias
//!    therefore identifies exactly one control catalog-wide.
//! 3. `ParameterRequirementSummary` is "[a] summary that indicates whether the control
//!    requires parameters, accepts optional parameters, or does not support parameters":
//!    a function of the control's own `Parameters` list, not an independent fact.
//!
//! Each test is a TABLE whose rows are every control the service serves, crossed with
//! every designator family and every implementation surface the catalog publishes —
//! never one entry picked by hand.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{drain, Emulator};

/// The implementation types this catalog publishes controls for, and whether Control
/// Tower deploys a control of that type (which is what a `CT.…` designator marks).
/// Source: the `ImplementationType` of the controls the catalog serves, each a published
/// identifier of the implementing surface; the modeled shape itself is an open string
/// (`smithy_query(query="shape", shape="ImplementationType")`).
const IMPLEMENTATION_SURFACES: [(&str, bool); 4] = [
    ("AWS::Config::ConfigRule", false),
    ("AWS::SecurityHub::SecurityControl", false),
    ("AWS::Organizations::Policy::SERVICE_CONTROL_POLICY", true),
    ("AWS::CloudFormation::Type::HOOK", true),
];

/// The designator prefix a Security Hub control is published under, as the `Aliases`
/// documentation's `SH.S3.1` example spells it.
const SECURITY_HUB_PREFIX: &str = "SH.";
/// The designator prefix of a control Amazon Web Services Control Tower publishes.
const CONTROL_TOWER_PREFIX: &str = "AWS-GR_";
/// The designator prefix of a catalog designator issued for an AWS Config rule.
const CONFIG_PREFIX: &str = "CONFIG.";
/// The designator prefix of a catalog designator issued for a Control Tower control.
const CONTROL_TOWER_DESIGNATOR_PREFIX: &str = "CT.";

/// The `Aliases` of a control summary, as strings.
fn aliases(control: &Value) -> Vec<String> {
    control["Aliases"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|alias| alias.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every control the catalog publishes, read over the wire.
async fn published_controls(emulator: &Emulator) -> Vec<Value> {
    let controls = drain(emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: ListControls must publish the catalog's controls"
    );
    controls
}

/// Every designator names the control on the surface that issues it, and no two
/// controls are published under the same designator.
#[tokio::test]
async fn every_alias_names_the_control_on_the_surface_that_issues_it() {
    let emulator = Emulator::new();
    let controls = published_controls(&emulator).await;

    let control_tower_deployed: BTreeMap<&str, bool> =
        IMPLEMENTATION_SURFACES.iter().copied().collect();
    let mut owner_of_alias: BTreeMap<String, String> = BTreeMap::new();
    let mut families_seen: BTreeSet<&str> = BTreeSet::new();

    for control in &controls {
        let arn = control["Arn"].as_str().expect("a control carries its ARN");
        let implementation_type = control["Implementation"]["Type"].as_str().unwrap_or_default();
        let identifier = control["Implementation"]["Identifier"].as_str();

        for alias in aliases(control) {
            // An alias identifies one control: whichever family it belongs to, no
            // second control may be published under it.
            if let Some(first) = owner_of_alias.insert(alias.clone(), arn.to_string()) {
                panic!(
                    "case alias {alias}: expected one control to carry it, got both {first} \
                     and {arn}"
                );
            }

            if let Some(security_hub_id) = alias.strip_prefix(SECURITY_HUB_PREFIX) {
                families_seen.insert(SECURITY_HUB_PREFIX);
                assert_eq!(
                    implementation_type, "AWS::SecurityHub::SecurityControl",
                    "case alias {alias} on {arn}: a Security Hub designator belongs to a \
                     Security Hub control, got implementation type {implementation_type}"
                );
                assert_eq!(
                    identifier,
                    Some(security_hub_id),
                    "case alias {alias} on {arn}: the designator must name this control's \
                     own Security Hub id, got {identifier:?}"
                );
            } else if alias.starts_with(CONTROL_TOWER_PREFIX) {
                families_seen.insert(CONTROL_TOWER_PREFIX);
                // The designator IS the control's Control Tower identifier, so the
                // Control Tower ARN form built from it resolves to this same control.
                let requested = format!("arn:aws:controltower:us-east-1::control/{alias}");
                let resolved = emulator
                    .ok("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
                    .await;
                assert_eq!(
                    resolved["Arn"].as_str(),
                    Some(arn),
                    "case alias {alias} on {arn}: a Control Tower designator must resolve \
                     to the control that carries it, got {resolved:?}"
                );
            } else if alias.starts_with(CONFIG_PREFIX) {
                families_seen.insert(CONFIG_PREFIX);
                assert_eq!(
                    implementation_type, "AWS::Config::ConfigRule",
                    "case alias {alias} on {arn}: a Config designator belongs to a control \
                     an AWS Config rule implements, got implementation type \
                     {implementation_type}"
                );
            } else if alias.starts_with(CONTROL_TOWER_DESIGNATOR_PREFIX) {
                families_seen.insert(CONTROL_TOWER_DESIGNATOR_PREFIX);
                assert_eq!(
                    control_tower_deployed.get(implementation_type),
                    Some(&true),
                    "case alias {alias} on {arn}: a CT designator belongs to a control \
                     Control Tower deploys, got implementation type {implementation_type}"
                );
            } else {
                panic!(
                    "case alias {alias} on {arn}: no published designator family issues it"
                );
            }
        }

        // The rule holds in the other direction too: a Security Hub control IS
        // published under its own designator.
        if implementation_type == "AWS::SecurityHub::SecurityControl" {
            let expected = format!(
                "{SECURITY_HUB_PREFIX}{}",
                identifier.expect("a Security Hub control carries its control id")
            );
            assert!(
                aliases(control).contains(&expected),
                "case {arn}: expected the Security Hub designator {expected}, got {:?}",
                aliases(control)
            );
        }
    }

    // Every family the rule distinguishes is exercised by the catalog, so no branch
    // above is asserted vacuously.
    for family in [
        SECURITY_HUB_PREFIX,
        CONTROL_TOWER_PREFIX,
        CONFIG_PREFIX,
        CONTROL_TOWER_DESIGNATOR_PREFIX,
    ] {
        assert!(
            families_seen.contains(family),
            "case family {family}: the catalog must publish a designator of this family \
             for the rule to be tested, saw {families_seen:?}"
        );
    }
}

/// `ParameterRequirementSummary` summarises the control's own `Parameters`, over every
/// control the catalog publishes — and the catalog exercises all three modeled values.
#[tokio::test]
async fn parameter_requirement_summary_summarises_the_parameter_list() {
    let emulator = Emulator::new();
    let controls = published_controls(&emulator).await;
    let mut summaries_seen: BTreeSet<String> = BTreeSet::new();

    for summary in &controls {
        let arn = summary["Arn"].as_str().expect("a control carries its ARN");
        let control = emulator
            .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;
        let parameters = control["Parameters"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let requirements: Vec<&str> = parameters
            .iter()
            .map(|parameter| {
                parameter["Requirement"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {arn}: a parameter states its requirement"))
            })
            .collect();

        // The modeled `ParameterRequirementSummary` admits exactly these three values.
        let expected = if requirements.is_empty() {
            "NONE"
        } else if requirements.contains(&"REQUIRED") {
            "REQUIRED"
        } else {
            "OPTIONAL"
        };
        let actual = control["ParameterRequirementSummary"].as_str();
        assert_eq!(
            actual,
            Some(expected),
            "case {arn} with parameters {requirements:?}: expected \
             ParameterRequirementSummary {expected}, got {actual:?}"
        );
        // The summary a client reads from the list page is the same summary.
        assert_eq!(
            summary["ParameterRequirementSummary"].as_str(),
            actual,
            "case {arn}: ListControls and GetControl must report one summary"
        );
        summaries_seen.insert(expected.to_string());
    }

    for value in ["NONE", "OPTIONAL", "REQUIRED"] {
        assert!(
            summaries_seen.contains(value),
            "case {value}: the catalog must publish a control summarised this way for the \
             rule to be tested, saw {summaries_seen:?}"
        );
    }
}

/// An implementation identifier does not identify a control: every control carrying it
/// is published, each under its own ARN and designators, and the `Identifiers` filter
/// answers with all of them.
#[tokio::test]
async fn an_implementation_identifier_can_be_shared_by_several_published_controls() {
    let emulator = Emulator::new();
    let controls = published_controls(&emulator).await;

    let mut by_identifier: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for control in &controls {
        if let Some(identifier) = control["Implementation"]["Identifier"].as_str() {
            by_identifier
                .entry(identifier.to_string())
                .or_default()
                .push(control.clone());
        }
    }
    assert!(
        !by_identifier.is_empty(),
        "case catalog: controls must report their implementation identifiers"
    );

    let mut shared_identifiers = 0usize;
    for (identifier, expected) in &by_identifier {
        let body = format!(r#"{{"Filter":{{"Implementations":{{"Identifiers":["{identifier}"]}}}}}}"#);
        // Page size 1 as well, so a shared identifier is walked across pages.
        for page_size in [None, Some(1u32)] {
            let filtered = drain(
                &emulator,
                "/list-controls",
                "Controls",
                page_size,
                &body,
            )
            .await;
            assert_eq!(
                &filtered,
                expected,
                "case identifier {identifier} maxResults={page_size:?}: expected the {} \
                 control(s) that carry it, got {filtered:?}",
                expected.len()
            );
        }

        if expected.len() > 1 {
            shared_identifiers += 1;
            // Each of them is its own published control: its own ARN, its own
            // designators, and readable on its own.
            let arns: BTreeSet<&str> = expected
                .iter()
                .filter_map(|control| control["Arn"].as_str())
                .collect();
            assert_eq!(
                arns.len(),
                expected.len(),
                "case identifier {identifier}: each control that carries it must have its \
                 own ARN, got {arns:?}"
            );
            for control in expected {
                let arn = control["Arn"].as_str().expect("a control carries its ARN");
                let read_back = emulator
                    .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
                    .await;
                assert_eq!(
                    read_back["Arn"].as_str(),
                    Some(arn),
                    "case {arn}: a control sharing an implementation must still be \
                     readable by its own ARN, got {read_back:?}"
                );
                assert_eq!(
                    read_back["Implementation"]["Identifier"].as_str(),
                    Some(identifier.as_str()),
                    "case {arn}: expected implementation identifier {identifier}, got \
                     {read_back:?}"
                );
            }
        }
    }
    assert!(
        shared_identifiers >= 1,
        "case catalog: at least one implementation identifier must back more than one \
         published control for this rule to be tested"
    );
}

/// Every implementation surface the catalog publishes controls for is served, and each
/// control reports the surface's own identifier where that surface issues one.
#[tokio::test]
async fn every_published_implementation_surface_is_served() {
    let emulator = Emulator::new();
    let controls = published_controls(&emulator).await;

    for (implementation_type, _) in IMPLEMENTATION_SURFACES {
        let body =
            format!(r#"{{"Filter":{{"Implementations":{{"Types":["{implementation_type}"]}}}}}}"#);
        let filtered = drain(&emulator, "/list-controls", "Controls", Some(5), &body).await;
        let expected: Vec<&Value> = controls
            .iter()
            .filter(|control| {
                control["Implementation"]["Type"].as_str() == Some(implementation_type)
            })
            .collect();
        assert!(
            !expected.is_empty(),
            "case type {implementation_type}: the catalog must publish a control \
             implemented this way"
        );
        assert_eq!(
            filtered.len(),
            expected.len(),
            "case type {implementation_type}: expected {} control(s), got {filtered:?}",
            expected.len()
        );
        for control in filtered {
            let arn = control["Arn"].as_str().unwrap_or_default();
            assert_eq!(
                control["Implementation"]["Type"].as_str(),
                Some(implementation_type),
                "case {arn}: the filter returned a control of another surface"
            );
            // A service control policy IS the control, so it has no per-control
            // identifier; every other surface issues one.
            if implementation_type == "AWS::Organizations::Policy::SERVICE_CONTROL_POLICY" {
                assert!(
                    control["Implementation"]["Identifier"].is_null(),
                    "case {arn}: a service control policy has no identifier of its own, \
                     got {control:?}"
                );
            } else {
                assert!(
                    control["Implementation"]["Identifier"]
                        .as_str()
                        .is_some_and(|identifier| !identifier.is_empty()),
                    "case {arn}: expected the implementing surface's own identifier, got \
                     {control:?}"
                );
            }
        }
    }
}
