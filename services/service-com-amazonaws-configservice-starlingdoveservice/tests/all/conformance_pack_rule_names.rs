// SPDX-License-Identifier: CC0-1.0

//! The names Config gives the Config rules a conformance pack deploys.
//!
//! The rule pinned here: a rule name CONFIG ITSELF generates stays within the tightest
//! published bound on a member that names a Config rule — the 64 characters
//! `StringWithCharLimit64` allows, which `GetComplianceDetailsByConfigRule` and
//! `DeleteEvaluationResults` take the name as — so a rule a conformance pack deployed is
//! always readable through the compliance paths of the same API that generated its name.
//! The generated name is still the declared name suffixed with the pack's own id, which
//! is what keeps two packs declaring one rule name from deploying over each other.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of declared
//! rule names rather than one recorded template: the rows differ along the LENGTH of the
//! declared name, from a short one through the longest that fits beside the suffix to one
//! far past the ceiling, and along the length the pack NAME contributes.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER, GENERATED_CONFIG_RULE_NAME_MAX_LENGTH,
};

use crate::config_setup_and_rule_loop::Emulator;

/// The resource type a conformance pack template declares a Config rule as, from
/// `PutConformancePack`'s own model doc.
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";

/// A Config managed rule identifier a declared rule's `Source` names; the process check
/// is the managed rule whose evaluations are delivered from outside Config.
const PROCESS_CHECK: &str = "AWS_CONFIG_PROCESS_CHECK";

/// Long enough for the clock to have made a pack's deployment due, read from the service
/// itself so the test cannot drift from the span the service owes.
fn past_the_deployment() -> Duration {
    CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER + Duration::from_millis(200)
}

/// A conformance pack template declaring one Config rule named `rule_name`.
fn rule_template(rule_name: &str) -> String {
    format!(
        "Resources:\n  OnlyRule:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
         ConfigRuleName: {rule_name}\n      Source:\n        Owner: AWS\n        \
         SourceIdentifier: {PROCESS_CHECK}\n"
    )
}

/// The names of every Config rule this account holds.
async fn rule_names(case: &str, emulator: &Emulator) -> Vec<String> {
    let body = emulator.ok("DescribeConfigRules", &json!({})).await;
    body["ConfigRules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConfigRules, got {body}"))
        .iter()
        .map(|rule| {
            rule["ConfigRuleName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a rule carries a name, got {rule}"))
                .to_string()
        })
        .collect()
}

/// The id Config assigned the pack `name`.
async fn pack_id(case: &str, emulator: &Emulator, name: &str) -> String {
    let body = emulator
        .ok(
            "DescribeConformancePackStatus",
            &json!({ "ConformancePackNames": [name] }),
        )
        .await;
    body["ConformancePackStatusDetails"][0]["ConformancePackId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the pack id, got {body}"))
        .to_string()
}

#[tokio::test]
async fn a_pack_deployed_rule_name_is_one_the_compliance_reads_accept() {
    // (case, the pack's name, the rule name the template declares). The rows differ
    // along the length of the declared name — including the longest that still fits
    // whole beside the generated suffix, one past it, and one far past the 64-character
    // ceiling — and along the pack name's own length.
    let cases: [(&str, &str, String); 7] = [
        ("a short declared name", "pack", "versioning".to_string()),
        (
            "the recorded process-check shape",
            "sim2real-pack",
            "sim2real-process-check-776e61f4bb8d".to_string(),
        ),
        (
            "a declared name at the longest that fits whole",
            "pack",
            "d".repeat(GENERATED_CONFIG_RULE_NAME_MAX_LENGTH - 26),
        ),
        (
            "one character past what fits whole",
            "pack",
            "e".repeat(GENERATED_CONFIG_RULE_NAME_MAX_LENGTH - 25),
        ),
        (
            "a declared name at the 64-character ceiling itself",
            "pack",
            "f".repeat(GENERATED_CONFIG_RULE_NAME_MAX_LENGTH),
        ),
        (
            "a declared name at the 128 a ConfigRuleName allows",
            "pack",
            "g".repeat(128),
        ),
        (
            "a long pack name, which the rule name does not carry",
            &"h".repeat(200),
            "short-rule".to_string(),
        ),
    ];

    for (case, pack, declared) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConformancePack",
                &json!({
                    "ConformancePackName": pack,
                    "TemplateBody": rule_template(&declared),
                }),
            )
            .await;
        tokio::time::sleep(past_the_deployment()).await;

        let id = pack_id(case, &emulator, pack).await;
        let names = rule_names(case, &emulator).await;
        let [deployed] = names.as_slice() else {
            panic!("case {case:?}: expected exactly the pack's one rule, got {names:?}");
        };

        // The generated name is readable through the member with the tightest bound on
        // a rule name...
        assert!(
            deployed.chars().count() <= GENERATED_CONFIG_RULE_NAME_MAX_LENGTH,
            "case {case:?}: expected a name of at most \
             {GENERATED_CONFIG_RULE_NAME_MAX_LENGTH} characters, got {} in {deployed:?}",
            deployed.chars().count()
        );
        // ... and it is still the declared name, as much of it as fits, suffixed with
        // the pack's own id, so the rule is identifiable as that pack's.
        assert!(
            deployed.ends_with(&format!("-{id}")),
            "case {case:?}: expected the name to end with the pack id {id:?}, got \
             {deployed:?}"
        );
        let kept = deployed
            .strip_suffix(&format!("-{id}"))
            .unwrap_or_else(|| panic!("case {case:?}: the suffix was just asserted"));
        assert!(
            !kept.is_empty() && declared.starts_with(kept),
            "case {case:?}: expected the name to open with the declared name {declared:?}, \
             got {deployed:?}"
        );

        // The compliance read that takes the name ACCEPTS it: that is the property the
        // ceiling exists for, and it is served rather than refused on a length
        // constraint.
        let details = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": deployed }),
            )
            .await;
        assert!(
            details["EvaluationResults"].is_array(),
            "case {case:?}: expected the compliance read of {deployed:?} to be served, got \
             {details}"
        );
    }
}

#[tokio::test]
async fn two_packs_declaring_one_rule_name_deploy_two_rules() {
    // The suffix is what distinguishes them, so shortening it must not let one pack's
    // rule land on another's.
    let emulator = Emulator::new();
    for pack in ["first-pack", "second-pack"] {
        emulator
            .ok(
                "PutConformancePack",
                &json!({
                    "ConformancePackName": pack,
                    "TemplateBody": rule_template("shared-rule-name"),
                }),
            )
            .await;
    }
    tokio::time::sleep(past_the_deployment()).await;

    let names = rule_names("two packs", &emulator).await;
    assert_eq!(
        names.len(),
        2,
        "two packs declaring one rule name deploy two rules, got {names:?}"
    );
    for pack in ["first-pack", "second-pack"] {
        let id = pack_id("two packs", &emulator, pack).await;
        assert!(
            names.iter().any(|name| name.ends_with(&format!("-{id}"))),
            "each pack holds its own rule, got {names:?} for the pack id {id:?}"
        );
    }
}
