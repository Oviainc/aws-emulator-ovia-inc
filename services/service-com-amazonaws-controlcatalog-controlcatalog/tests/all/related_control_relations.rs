// SPDX-License-Identifier: CC0-1.0

//! `ListControlMappings`: which relation a RELATED_CONTROL mapping reports.
//!
//! The rule these tests pin is one sentence: two published controls that are the same
//! check on two surfaces — one implementation (the same `Implementation.Type` and
//! `Implementation.Identifier`) behind both, so either one satisfies the common control
//! they both implement — relate as ALTERNATIVE, and a COMPLEMENTARY relation is
//! reported only for a pair of two DISTINCT checks that reinforce one another.
//!
//! So the tests are a TABLE over every related-control pair the catalog publishes, in
//! both directions and through both accepted spellings of a control's ARN, rather than
//! one pair written out by hand: the check behind each control and the common control it
//! implements are read from the wire (`GetControl`, the COMMON_CONTROL mapping) and the
//! relation is compared against what the rule says for that pair.
//!
//! The harness (`Emulator`, `drain`, `at_path`) is the one `control_catalog.rs` already
//! drives the service through — the same `MultiServiceRouter` front door.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{at_path, drain, Emulator};
use crate::control_mappings::{MAPPINGS_KEY, MAPPINGS_PATH, RELATION_TYPES};

/// The route `ListControls` is bound to, and the member carrying its items
/// (`POST /list-controls`, `Controls`).
const CONTROLS_PATH: &str = "/list-controls";
const CONTROLS_KEY: &str = "Controls";

/// The prefix of the Control Tower identifier a control Control Tower publishes is
/// designated by (`AWS-GR_DETECT_…`), which is also the alias the catalog publishes it
/// under (`smithy_query(query="doc", shape="ControlSummary", member="Aliases")`).
const CONTROL_TOWER_ALIAS_PREFIX: &str = "AWS-GR_";

/// What the catalog publishes about one control, as far as relating it to another goes.
#[derive(Debug)]
struct Published {
    /// The check behind the control: `Implementation.Type` together with the
    /// implementing surface's own `Identifier`. `None` where the surface publishes no
    /// per-control identifier (a service control policy), so no two of its controls can
    /// be shown to carry one check.
    check: Option<(String, String)>,
    /// The common control this control implements: its COMMON_CONTROL mapping.
    common_control_arn: String,
    /// Each control it relates to, with the relation reported: `ControlArn` ->
    /// `RelationType`.
    related: BTreeMap<String, String>,
    /// The designators the control is published under, which carry the Control Tower
    /// identifier of a control Control Tower publishes.
    aliases: Vec<String>,
}

/// The `ControlArn` -> `RelationType` map a set of reported mappings carries, reading
/// only its RELATED_CONTROL entries.
fn relations(mappings: &[Value]) -> BTreeMap<String, String> {
    let mut related: BTreeMap<String, String> = BTreeMap::new();
    for mapping in mappings {
        if at_path(mapping, &["MappingType"]) != Some("RELATED_CONTROL") {
            continue;
        }
        let target = at_path(mapping, &["Mapping", "RelatedControl", "ControlArn"])
            .unwrap_or_else(|| panic!("case mapping {mapping:?}: expected a related ControlArn"))
            .to_string();
        let relation = at_path(mapping, &["Mapping", "RelatedControl", "RelationType"])
            .unwrap_or_else(|| panic!("case mapping {mapping:?}: expected a RelationType"))
            .to_string();
        assert!(
            related.insert(target.clone(), relation).is_none(),
            "case mapping {mapping:?}: control {target} is related more than once"
        );
    }
    related
}

/// Read every published control over the wire, with the check behind it, the common
/// control it implements and the relations it reports.
async fn published(emulator: &Emulator) -> BTreeMap<String, Published> {
    let controls = drain(emulator, CONTROLS_PATH, CONTROLS_KEY, None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: expected the catalog to publish controls, got none"
    );

    let mut catalog: BTreeMap<String, Published> = BTreeMap::new();
    for summary in &controls {
        let arn = at_path(summary, &["Arn"])
            .unwrap_or_else(|| panic!("case control {summary:?}: expected an Arn"))
            .to_string();
        let read = emulator
            .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;
        let check = at_path(&read, &["Implementation", "Type"]).and_then(|kind| {
            at_path(&read, &["Implementation", "Identifier"])
                .map(|identifier| (kind.to_string(), identifier.to_string()))
        });
        let aliases: Vec<String> = read
            .get("Aliases")
            .and_then(Value::as_array)
            .map(|aliases| {
                aliases
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let mappings = drain(
            emulator,
            MAPPINGS_PATH,
            MAPPINGS_KEY,
            None,
            &format!(r#"{{"Filter":{{"ControlArns":["{arn}"]}}}}"#),
        )
        .await;
        let common_control_arn = mappings
            .iter()
            .find(|mapping| at_path(mapping, &["MappingType"]) == Some("COMMON_CONTROL"))
            .and_then(|mapping| at_path(mapping, &["Mapping", "CommonControl", "CommonControlArn"]))
            .unwrap_or_else(|| {
                panic!("case control {arn}: expected a COMMON_CONTROL mapping, got {mappings:?}")
            })
            .to_string();

        catalog.insert(
            arn,
            Published {
                check,
                common_control_arn,
                related: relations(&mappings),
                aliases,
            },
        );
    }
    catalog
}

/// Every related-control pair the catalog publishes reports the relation the rule gives
/// it: ALTERNATIVE where one implementation backs both controls, and an ALTERNATIVE
/// relation only between controls implementing one common control.
#[tokio::test]
async fn a_pair_of_controls_backed_by_one_check_relates_as_alternative() {
    let emulator = Emulator::new();
    let catalog = published(&emulator).await;

    // One row per reported relation: every pair, in both directions.
    let mut pairs = 0usize;
    let mut one_check_pairs = 0usize;
    let mut relations_seen: BTreeSet<&str> = BTreeSet::new();
    let mut alternative_checks: BTreeSet<&(String, String)> = BTreeSet::new();

    for (arn, control) in &catalog {
        for (related_arn, relation) in &control.related {
            pairs += 1;
            let other = catalog.get(related_arn).unwrap_or_else(|| {
                panic!("case pair {arn} / {related_arn}: the related control is not published")
            });
            assert!(
                RELATION_TYPES.contains(&relation.as_str()),
                "case pair {arn} / {related_arn}: relation {relation} is not a modeled \
                 ControlRelationType {RELATION_TYPES:?}"
            );
            relations_seen.insert(relation.as_str());

            // One implementation behind both controls is the same check on two surfaces:
            // either control satisfies the objective, so the pair is an ALTERNATIVE and
            // never a COMPLEMENTARY one, which would claim the second control adds
            // coverage the first does not already give.
            if control.check.is_some() && control.check == other.check {
                one_check_pairs += 1;
                assert_eq!(
                    relation,
                    "ALTERNATIVE",
                    "case pair {arn} / {related_arn} backed by the one check {:?}: expected \
                     ALTERNATIVE, got {relation}",
                    control.check
                );
            }

            // An ALTERNATIVE is only an alternative for something: the two controls must
            // implement the same common control for either to satisfy it.
            if relation == "ALTERNATIVE" {
                assert_eq!(
                    control.common_control_arn, other.common_control_arn,
                    "case pair {arn} / {related_arn} reported ALTERNATIVE: expected both to \
                     implement one common control, got {} and {}",
                    control.common_control_arn, other.common_control_arn
                );
                if let Some(check) = control.check.as_ref() {
                    alternative_checks.insert(check);
                }
            }

            // The catalog records a pair in both directions, and a relation is symmetric:
            // whichever side is queried reports the same relation.
            assert_eq!(
                other.related.get(arn),
                Some(relation),
                "case pair {related_arn} / {arn}: expected the reverse direction to report \
                 {relation}, got {:?}",
                other.related.get(arn)
            );
        }
    }

    // The rows above have to span the class: pairs of both relations, and more than one
    // pair sharing a single check, so neither assertion is vacuous.
    assert!(
        pairs >= 2,
        "case catalog: expected related-control pairs to compare, got {pairs}"
    );
    assert!(
        one_check_pairs >= 2,
        "case catalog: expected the catalog to publish a check on two surfaces in both \
         directions, got {one_check_pairs} such relation(s)"
    );
    assert!(
        alternative_checks.len() >= 2,
        "case catalog: expected ALTERNATIVE pairs spanning more than one check, got \
         {alternative_checks:?}"
    );
    for relation in ["ALTERNATIVE", "COMPLEMENTARY"] {
        assert!(
            relations_seen.contains(relation),
            "case catalog: expected a {relation} pair among the reported relations, got \
             {relations_seen:?}"
        );
    }
}

/// A control's relations are the same whichever accepted form of its ARN the filter
/// names it by — the catalog form or the Control Tower form, in any partition and
/// whatever Region the Control Tower ARN carries.
#[tokio::test]
async fn the_control_tower_arn_form_reports_the_same_relations() {
    let emulator = Emulator::new();
    let catalog = published(&emulator).await;

    let mut covered = 0usize;
    for (arn, control) in &catalog {
        for alias in control
            .aliases
            .iter()
            .filter(|alias| alias.starts_with(CONTROL_TOWER_ALIAS_PREFIX))
        {
            // Rows differ in the segments that are not part of a control's identity: the
            // partition, and the Region a Control Tower ARN carries.
            for (partition, region) in [("aws", "us-east-1"), ("aws", "eu-west-1")] {
                let requested = format!("arn:{partition}:controltower:{region}::control/{alias}");
                let mappings = drain(
                    &emulator,
                    MAPPINGS_PATH,
                    MAPPINGS_KEY,
                    Some(2),
                    &format!(
                        r#"{{"Filter":{{"ControlArns":["{requested}"],"MappingTypes":["RELATED_CONTROL"]}}}}"#
                    ),
                )
                .await;
                assert_eq!(
                    relations(&mappings),
                    control.related,
                    "case control {arn} filtered as {requested}: expected the relations the \
                     catalog reports for it, got {mappings:?}"
                );
                covered += 1;
            }
        }
    }
    assert!(
        covered >= 2,
        "case catalog: expected controls published under a Control Tower identifier, got \
         {covered} row(s)"
    );
}
