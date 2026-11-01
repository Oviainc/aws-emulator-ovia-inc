// SPDX-License-Identifier: CC0-1.0

//! What a published control's `Parameters` may name, and what the requirement summary
//! may rest on.
//!
//! The rules under test, read off the modeled shapes the members belong to
//! (`smithy_query(query="doc", shape="ControlParameter")`,
//! `smithy_query(query="operation", operation="GetControl")`):
//!
//! 1. A `ControlParameter` is a value the caller supplies when it ENABLES the control,
//!    and the shape documents the whole vocabulary — "Five types of control parameters
//!    are supported". Every name a control publishes in `Parameters` is therefore one
//!    of those five, whatever surface implements the control: a setting of the AWS
//!    Config rule, Security Hub control, policy or hook behind the control is that
//!    surface's own configuration, not a control parameter.
//! 2. `Parameters` "[r]eturns an array of ControlParameter objects that specify the
//!    parameters a control supports. An empty list is returned for controls that don't
//!    support parameters."
//! 3. `ParameterRequirementSummary` is a summary of exactly that list — "[u]se this
//!    field to determine whether you need to supply parameter values when you enable
//!    the control" — so it may only be `OPTIONAL` or `REQUIRED` on the strength of
//!    control parameters the vocabulary has a key for.
//!
//! Each test is a TABLE whose rows are every control the catalog publishes, crossed
//! with every ARN form `GetControl` accepts for it, rather than one control picked by
//! hand.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{drain, Emulator};

/// The control parameters a published control can support, spelled as the modeled
/// `ControlParameter` spells them. Source: that shape's own documentation, which
/// enumerates the five and nothing else (`smithy_query(query="doc",
/// shape="ControlParameter")`).
const DOCUMENTED_CONTROL_PARAMETERS: [&str; 5] = [
    "AllowedRegions",
    "ExemptedActions",
    "ExemptedPrincipalArns",
    "ExemptedResourceArns",
    "ExemptAssumeRoot",
];

/// The values the modeled `ControlParameterRequirement` admits
/// (`smithy_query(query="shape", shape="ControlParameterRequirement")`).
const DOCUMENTED_REQUIREMENTS: [&str; 2] = ["OPTIONAL", "REQUIRED"];

/// The designator prefix of a control Amazon Web Services Control Tower publishes,
/// which is also the opaque id of its `controltower` ARN form.
const CONTROL_TOWER_PREFIX: &str = "AWS-GR_";

/// Every control the catalog publishes, read over the wire.
async fn published_controls(emulator: &Emulator) -> Vec<Value> {
    let controls = drain(emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: ListControls must publish the catalog's controls"
    );
    controls
}

/// Every ARN form `GetControl` accepts for a control: its own `controlcatalog` ARN,
/// plus the `controltower` form of each Control Tower designator it carries.
fn read_forms(control: &Value) -> Vec<String> {
    let arn = control["Arn"]
        .as_str()
        .expect("a control carries its ARN")
        .to_string();
    let mut forms = vec![arn];
    for alias in control["Aliases"].as_array().cloned().unwrap_or_default() {
        if let Some(alias) = alias.as_str() {
            if alias.starts_with(CONTROL_TOWER_PREFIX) {
                forms.push(format!("arn:aws:controltower:us-east-1::control/{alias}"));
            }
        }
    }
    forms
}

/// The `(Name, Requirement)` pairs of a control's `Parameters`, in the order served.
fn parameters(control: &Value, case: &str) -> Vec<(String, String)> {
    let served = control["Parameters"].as_array().unwrap_or_else(|| {
        panic!("case {case}: expected a Parameters array, got {:?}", control["Parameters"])
    });
    served
        .iter()
        .map(|parameter| {
            let name = parameter["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a parameter states its name"))
                .to_string();
            let requirement = parameter["Requirement"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: parameter {name} states its requirement")
                })
                .to_string();
            (name, requirement)
        })
        .collect()
}

/// Every parameter a control publishes is one of the control parameters the model
/// documents, whichever ARN form the control is read by.
#[tokio::test]
async fn every_published_control_parameter_is_one_the_model_documents() {
    let emulator = Emulator::new();
    let summaries = published_controls(&emulator).await;
    let documented: BTreeSet<&str> = DOCUMENTED_CONTROL_PARAMETERS.iter().copied().collect();
    let requirements: BTreeSet<&str> = DOCUMENTED_REQUIREMENTS.iter().copied().collect();

    let mut names_seen: BTreeSet<String> = BTreeSet::new();
    let mut controls_with_parameters = 0usize;
    let mut controls_without_parameters = 0usize;
    let mut surfaces_seen: BTreeSet<String> = BTreeSet::new();

    for summary in &summaries {
        let forms = read_forms(summary);
        let mut served_per_form: Vec<Vec<(String, String)>> = Vec::new();

        for requested in &forms {
            let control = emulator
                .ok("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
                .await;
            let case = format!("control read as {requested}");
            let served = parameters(&control, &case);

            let mut names: BTreeSet<&str> = BTreeSet::new();
            for (name, requirement) in &served {
                assert!(
                    documented.contains(name.as_str()),
                    "case {case}: a control parameter must be one the model documents \
                     ({documented:?}), got {name:?} \u{2014} the implementing surface's own \
                     settings are not control parameters"
                );
                assert!(
                    requirements.contains(requirement.as_str()),
                    "case {case}: parameter {name} must state a modeled requirement \
                     ({requirements:?}), got {requirement:?}"
                );
                assert!(
                    names.insert(name.as_str()),
                    "case {case}: parameter {name} must be published once, got {served:?}"
                );
                names_seen.insert(name.clone());
            }

            if served.is_empty() {
                // "An empty list is returned for controls that don't support
                // parameters" — the member is served, not omitted.
                controls_without_parameters += 1;
            } else {
                controls_with_parameters += 1;
            }
            served_per_form.push(served);
        }

        // The parameters are a property of the control, not of the ARN it was asked
        // for, so every accepted form reports the same list.
        let first = &served_per_form[0];
        for (requested, served) in forms.iter().zip(&served_per_form) {
            assert_eq!(
                served, first,
                "case control read as {requested}: expected the same parameters as \
                 {}, got {served:?}",
                forms[0]
            );
        }

        if let Some(surface) = summary["Implementation"]["Type"].as_str() {
            surfaces_seen.insert(surface.to_string());
        }
    }

    // None of the assertions above is vacuous: the catalog publishes controls that
    // support control parameters and controls that support none, over more than one
    // implementation surface.
    assert!(
        controls_with_parameters >= 1,
        "case catalog: at least one control must support a control parameter for the \
         rule to be tested, saw {controls_with_parameters}"
    );
    assert!(
        controls_without_parameters >= 1,
        "case catalog: at least one control must support no parameter for the rule to \
         be tested, saw {controls_without_parameters}"
    );
    assert!(
        names_seen.len() >= 2,
        "case catalog: the published parameter names must span more than one of the \
         documented five for the rule to be tested, saw {names_seen:?}"
    );
    assert!(
        surfaces_seen.len() >= 2,
        "case catalog: the rule holds across implementation surfaces, so more than one \
         must be published, saw {surfaces_seen:?}"
    );
}

/// `ParameterRequirementSummary` rests on control parameters only: summarising the
/// documented names a control publishes reproduces the summary it serves, on both the
/// list page and the control itself.
#[tokio::test]
async fn the_requirement_summary_rests_on_documented_control_parameters_only() {
    let emulator = Emulator::new();
    let summaries = published_controls(&emulator).await;
    let documented: BTreeSet<&str> = DOCUMENTED_CONTROL_PARAMETERS.iter().copied().collect();

    let mut summaries_seen: BTreeMap<String, usize> = BTreeMap::new();

    for summary in &summaries {
        let arn = summary["Arn"].as_str().expect("a control carries its ARN");
        let control = emulator
            .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;
        let case = format!("control read as {arn}");
        let served = parameters(&control, &case);

        // Only a parameter the vocabulary has a key for can oblige the caller to
        // supply a value when enabling the control.
        let control_parameters: Vec<&(String, String)> = served
            .iter()
            .filter(|(name, _)| documented.contains(name.as_str()))
            .collect();
        let expected = if control_parameters.is_empty() {
            "NONE"
        } else if control_parameters
            .iter()
            .any(|(_, requirement)| requirement == "REQUIRED")
        {
            "REQUIRED"
        } else {
            "OPTIONAL"
        };

        let actual = control["ParameterRequirementSummary"].as_str();
        assert_eq!(
            actual,
            Some(expected),
            "case {case} with control parameters {control_parameters:?} out of \
             {served:?}: expected ParameterRequirementSummary {expected}, got {actual:?}"
        );
        assert_eq!(
            summary["ParameterRequirementSummary"].as_str(),
            actual,
            "case {case}: ListControls and GetControl must report one summary"
        );
        *summaries_seen.entry(expected.to_string()).or_default() += 1;
    }

    // Each modeled summary value is exercised, so no branch above is vacuous.
    for value in ["NONE", "OPTIONAL", "REQUIRED"] {
        assert!(
            summaries_seen.contains_key(value),
            "case {value}: the catalog must publish a control summarised this way for \
             the rule to be tested, saw {summaries_seen:?}"
        );
    }
}
