// SPDX-License-Identifier: CC0-1.0

//! What a published control's `Parameters` may name, and what the requirement summary
//! rests on.
//!
//! The rules under test, read off the modeled shapes the members belong to
//! (`smithy_query(query="doc", shape="ControlParameter")`,
//! `smithy_query(query="operation", operation="GetControl")`):
//!
//! 1. A `ControlParameter` is a value the caller supplies when it ENABLES the control,
//!    so which parameters a control supports is a property of the SURFACE that
//!    implements it, not of the control's own row in the catalog: a control an AWS
//!    Config managed rule implements publishes that rule's own input parameters — the
//!    recorded catalog response publishes a Config-rule control's parameter under the
//!    rule's own name, outside the five the shape's documentation enumerates — so every
//!    control over one rule publishes one and the same parameter set, while a control
//!    deployed as a policy or a hook publishes the exemption parameters the shape does
//!    enumerate ("Five types of control parameters are supported").
//! 2. `Parameters` "[r]eturns an array of ControlParameter objects that specify the
//!    parameters a control supports. An empty list is returned for controls that don't
//!    support parameters."
//! 3. `ParameterRequirementSummary` is a summary of exactly that list — "[u]se this
//!    field to determine whether you need to supply parameter values when you enable
//!    the control" — so it is `NONE` for a control that publishes no parameter,
//!    `REQUIRED` when one must be supplied and `OPTIONAL` otherwise.
//!
//! Each test is a TABLE whose rows are every control the catalog publishes, crossed
//! with every ARN form `GetControl` accepts for it, rather than one control picked by
//! hand.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{drain, Emulator};

/// The EXEMPTION parameters a control deployed as a policy or a hook can support,
/// spelled as the modeled `ControlParameter` spells them. Source: that shape's own
/// documentation, which enumerates the five (`smithy_query(query="doc",
/// shape="ControlParameter")`).
const DOCUMENTED_EXEMPTION_PARAMETERS: [&str; 5] = [
    "AllowedRegions",
    "ExemptedActions",
    "ExemptedPrincipalArns",
    "ExemptedResourceArns",
    "ExemptAssumeRoot",
];

/// The implementation types of a control Amazon Web Services Control Tower DEPLOYS — a
/// service control policy or a CloudFormation hook — which are the controls the
/// exemption vocabulary above applies to. Source: the `ImplementationType` values the
/// catalog publishes.
const DEPLOYED_CONTROL_SURFACES: [&str; 2] = [
    "AWS::Organizations::Policy::SERVICE_CONTROL_POLICY",
    "AWS::CloudFormation::Type::HOOK",
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

/// Every parameter a control publishes belongs to the surface that implements the
/// control: the exemption vocabulary for a control Control Tower deploys, and the
/// implementing rule's own parameter set — one and the same for every control over that
/// rule — for a control an AWS Config managed rule implements. The list is a property of
/// the control, so it does not depend on the ARN form the control is read by.
#[tokio::test]
async fn every_published_control_parameter_belongs_to_the_implementing_surface() {
    let emulator = Emulator::new();
    let summaries = published_controls(&emulator).await;
    let exemptions: BTreeSet<&str> = DOCUMENTED_EXEMPTION_PARAMETERS.iter().copied().collect();
    let deployed: BTreeSet<&str> = DEPLOYED_CONTROL_SURFACES.iter().copied().collect();
    let requirements: BTreeSet<&str> = DOCUMENTED_REQUIREMENTS.iter().copied().collect();

    let mut names_seen: BTreeSet<String> = BTreeSet::new();
    let mut controls_with_parameters = 0usize;
    let mut controls_without_parameters = 0usize;
    let mut surfaces_seen: BTreeSet<String> = BTreeSet::new();
    // The parameter set published for each implementation identifier, to check that two
    // controls over one implementation publish one set.
    let mut per_implementation: BTreeMap<String, (String, Vec<(String, String)>)> =
        BTreeMap::new();

    for summary in &summaries {
        let forms = read_forms(summary);
        let implementation_type = summary["Implementation"]["Type"]
            .as_str()
            .unwrap_or_default()
            .to_string();
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
                    !name.is_empty(),
                    "case {case}: a published parameter must be named, got {served:?}"
                );
                if deployed.contains(implementation_type.as_str()) {
                    assert!(
                        exemptions.contains(name.as_str()),
                        "case {case}: a control Control Tower deploys takes the exemption \
                         parameters the model documents ({exemptions:?}), got {name:?}"
                    );
                }
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

        // Two controls over ONE implementation are parameterised by that implementation,
        // so they publish one and the same parameter set.
        let arn = summary["Arn"].as_str().unwrap_or_default().to_string();
        if let Some(identifier) = summary["Implementation"]["Identifier"].as_str() {
            let implementation = format!("{implementation_type}/{identifier}");
            if let Some((other, expected)) = per_implementation.get(&implementation) {
                assert_eq!(
                    first, expected,
                    "case {arn} over {implementation}: expected the parameters {other} \
                     publishes for that implementation ({expected:?}), got {first:?}"
                );
            } else {
                per_implementation.insert(implementation, (arn, first.clone()));
            }
        }

        if !implementation_type.is_empty() {
            surfaces_seen.insert(implementation_type);
        }
    }

    // None of the assertions above is vacuous: the catalog publishes controls that
    // support control parameters and controls that support none, over more than one
    // implementation surface, including a surface the exemption vocabulary governs.
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
        "case catalog: the published parameter names must span more than one name for \
         the rule to be tested, saw {names_seen:?}"
    );
    assert!(
        surfaces_seen.iter().any(|surface| deployed.contains(surface.as_str())),
        "case catalog: a control Control Tower deploys must be published for the \
         exemption rule to be tested, saw {surfaces_seen:?}"
    );
    assert!(
        surfaces_seen.len() >= 2,
        "case catalog: the rule holds across implementation surfaces, so more than one \
         must be published, saw {surfaces_seen:?}"
    );
}

/// `ParameterRequirementSummary` summarises exactly the control's own `Parameters`, on
/// both the list page and the control itself.
#[tokio::test]
async fn the_requirement_summary_summarises_the_published_parameters() {
    let emulator = Emulator::new();
    let summaries = published_controls(&emulator).await;

    let mut summaries_seen: BTreeMap<String, usize> = BTreeMap::new();

    for summary in &summaries {
        let arn = summary["Arn"].as_str().expect("a control carries its ARN");
        let control = emulator
            .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;
        let case = format!("control read as {arn}");
        let served = parameters(&control, &case);

        // The modeled summary says whether the caller must supply a value when it
        // enables the control, over the whole published list.
        let expected = if served.is_empty() {
            "NONE"
        } else if served
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
            "case {case} with parameters {served:?}: expected \
             ParameterRequirementSummary {expected}, got {actual:?}"
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
