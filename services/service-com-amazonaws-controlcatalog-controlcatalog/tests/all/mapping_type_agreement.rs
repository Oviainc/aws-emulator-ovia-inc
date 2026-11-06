// SPDX-License-Identifier: CC0-1.0

//! `ListControlMappings`: a reported mapping's `MappingType` names the `Mapping` union
//! member it carries, and what that member points at is served by the operation that
//! owns it.
//!
//! The rule pinned here is one sentence: for every control the catalog publishes, the
//! operation reports that control's whole mapping set, each entry carrying exactly one
//! member of the modeled `Mapping` union, the `MappingType` that names that member, and
//! a target the catalog itself serves — so the `MappingTypes` filter dimension selects
//! on the same fact the entry reports. Each test is therefore a TABLE whose rows are
//! every published control crossed with every value the modeled `MappingType` enum
//! declares, driven over the wire through the same `MultiServiceRouter` front door
//! `control_catalog.rs` uses, and the targets are read back through `ListCommonControls`
//! and `GetControl` rather than trusted.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;

use crate::control_catalog::{drain, Emulator};
use crate::control_mappings::{MAPPINGS_KEY, MAPPINGS_PATH, MAPPING_TYPES, RELATION_TYPES};

/// The `MappingType` each member of the modeled `Mapping` union is reported under, and
/// the members that member's details structure declares `@required`
/// (`smithy_query(query="shape",
/// shape="Mapping,FrameworkMappingDetails,CommonControlMappingDetails,RelatedControlMappingDetails")`:
/// the union declares `Framework`, `CommonControl` and `RelatedControl`; `Framework`
/// requires `Name` and `Item`, `CommonControl` requires `CommonControlArn`, and
/// `RelatedControl` requires `RelationType` with `ControlArn` optional).
const UNION_MEMBERS: [(&str, &str, &[&str]); 3] = [
    ("Framework", "FRAMEWORK", &["Name", "Item"]),
    ("CommonControl", "COMMON_CONTROL", &["CommonControlArn"]),
    ("RelatedControl", "RELATED_CONTROL", &["RelationType"]),
];

/// `FrameworkName`'s and `FrameworkItem`'s modeled `@length` bounds
/// (`smithy_query(query="shape", shape="FrameworkMappingDetails")`).
const FRAMEWORK_BOUNDS: (usize, usize) = (3, 250);

/// The route and item member of the two operations a mapping's target belongs to.
const COMMON_CONTROLS_PATH: &str = "/common-controls";
const COMMON_CONTROLS_KEY: &str = "CommonControls";
const GET_CONTROL_PATH: &str = "/get-control";

/// The single `Mapping` union member a reported mapping carries, with the case text to
/// name it by when an assertion fails.
fn union_member(case: &str, mapping: &Value) -> String {
    let members: Vec<String> = mapping["Mapping"]
        .as_object()
        .map(|details| details.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(
        members.len(),
        1,
        "case {case}: a Mapping union carries exactly one member, got {members:?} in \
         {mapping:?}"
    );
    members
        .into_iter()
        .next()
        .expect("the assertion above proves there is one member")
}

/// The `MappingType` the model reports `member` under.
fn mapping_type_of(case: &str, member: &str) -> &'static str {
    UNION_MEMBERS
        .iter()
        .find(|(candidate, _, _)| *candidate == member)
        .map(|(_, mapping_type, _)| *mapping_type)
        .unwrap_or_else(|| {
            panic!("case {case}: {member} is not a member of the modeled Mapping union")
        })
}

/// Every mapping the operation reports for `control_arn`, narrowed to `mapping_type`
/// when one is given.
async fn mappings_of(
    emulator: &Emulator,
    control_arn: &str,
    mapping_type: Option<&str>,
) -> Vec<Value> {
    let body = match mapping_type {
        Some(mapping_type) => format!(
            r#"{{"Filter":{{"ControlArns":["{control_arn}"],"MappingTypes":["{mapping_type}"]}}}}"#
        ),
        None => format!(r#"{{"Filter":{{"ControlArns":["{control_arn}"]}}}}"#),
    };
    drain(emulator, MAPPINGS_PATH, MAPPINGS_KEY, None, &body).await
}

/// Every reported mapping carries one union member, reports the `MappingType` that
/// names it, and is selected by exactly that value of the `MappingTypes` filter.
#[tokio::test]
async fn a_reported_mapping_names_the_union_member_it_carries() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    // One row per published control, crossed with every modeled MappingType: the type
    // reported for a mapping, the members its details carry and the filter value that
    // selects it all have to agree, for every control the catalog publishes rather than
    // for one picked by hand.
    let mut kinds_served: BTreeSet<&'static str> = BTreeSet::new();
    let mut reported = 0usize;
    for control in catalog.controls.values() {
        let case = format!("control {}", control.arn);
        let mappings = mappings_of(&emulator, &control.arn, None).await;
        assert!(
            !mappings.is_empty(),
            "case {case}: a published control must report the mapping set the catalog \
             records for it, got none"
        );
        reported += mappings.len();

        for mapping in &mappings {
            assert_eq!(
                mapping["ControlArn"].as_str(),
                Some(control.arn.as_str()),
                "case {case}: a mapping reported under this control's filter must name it, \
                 got {mapping:?}"
            );
            let member = union_member(&case, mapping);
            let expected = mapping_type_of(&case, &member);
            assert_eq!(
                mapping["MappingType"].as_str(),
                Some(expected),
                "case {case} member {member}: expected MappingType {expected}, got {mapping:?}"
            );
            let required = UNION_MEMBERS
                .iter()
                .find(|(candidate, _, _)| *candidate == member)
                .map(|(_, _, required)| *required)
                .unwrap_or_default();
            for name in required {
                let value = mapping["Mapping"][&member][*name].as_str();
                assert!(
                    value.is_some_and(|value| !value.is_empty()),
                    "case {case} member {member}: {name} is required, got {mapping:?}"
                );
            }
            kinds_served.insert(expected);
        }

        // The filter dimension selects on the same fact: narrowing to a MappingType
        // reports exactly the entries whose union member that type names, and nothing
        // else — in the same order the unnarrowed sequence reports them.
        for mapping_type in MAPPING_TYPES {
            let expected: Vec<Value> = mappings
                .iter()
                .filter(|mapping| {
                    mapping_type_of(&case, &union_member(&case, mapping)) == mapping_type
                })
                .cloned()
                .collect();
            let narrowed = mappings_of(&emulator, &control.arn, Some(mapping_type)).await;
            assert_eq!(
                narrowed,
                expected,
                "case {case} type {mapping_type}: expected the {} mapping(s) carrying that \
                 union member, got {narrowed:?}",
                expected.len()
            );
        }
    }

    assert!(
        reported >= catalog.controls.len(),
        "case catalog: expected at least one mapping per published control, got {reported} \
         for {} control(s)",
        catalog.controls.len()
    );
    for mapping_type in MAPPING_TYPES {
        assert!(
            kinds_served.contains(mapping_type),
            "case catalog: the catalog must serve mappings of every modeled type, {mapping_type} \
             was never reported (served {kinds_served:?})"
        );
    }
}

/// What a mapping points at is served by the operation that owns it: a common control
/// `ListCommonControls` returns, or a control `GetControl` reads — and a relation is
/// reported from both sides.
#[tokio::test]
async fn a_mapping_target_resolves_through_the_operation_that_owns_it() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    let common_controls: BTreeSet<String> = drain(
        &emulator,
        COMMON_CONTROLS_PATH,
        COMMON_CONTROLS_KEY,
        Some(25),
        "{}",
    )
    .await
    .iter()
    .filter_map(|common_control| common_control["Arn"].as_str().map(str::to_string))
    .collect();
    assert!(
        !common_controls.is_empty(),
        "case catalog: ListCommonControls must serve the common controls mappings point at"
    );

    // One row per published control: every target it reports is checked against the
    // operation that owns that kind of resource, so a mapping cannot name a resource
    // the emulator does not serve.
    let mut checked: BTreeMap<&'static str, usize> = BTreeMap::new();
    for control in catalog.controls.values() {
        let case = format!("control {}", control.arn);
        for mapping in mappings_of(&emulator, &control.arn, None).await {
            let member = union_member(&case, &mapping);
            let details = &mapping["Mapping"][&member];
            *checked.entry(mapping_type_of(&case, &member)).or_default() += 1;
            match member.as_str() {
                "CommonControl" => {
                    let arn = details["CommonControlArn"].as_str().unwrap_or_default();
                    assert!(
                        common_controls.contains(arn),
                        "case {case}: the COMMON_CONTROL target {arn} is not served by \
                         ListCommonControls"
                    );
                }
                "RelatedControl" => {
                    let relation = details["RelationType"].as_str().unwrap_or_default();
                    assert!(
                        RELATION_TYPES.contains(&relation),
                        "case {case}: {relation} is not a modeled ControlRelationType \
                         {RELATION_TYPES:?}"
                    );
                    let arn = details["ControlArn"].as_str().unwrap_or_default();
                    let read = emulator
                        .ok(GET_CONTROL_PATH, &format!(r#"{{"ControlArn":"{arn}"}}"#))
                        .await;
                    assert_eq!(
                        read["Arn"].as_str(),
                        Some(arn),
                        "case {case}: GetControl must read the RELATED_CONTROL target {arn}, \
                         got {read:?}"
                    );

                    // A relation holds between two controls, so the control on the other
                    // side reports it too: a client filtering on either ARN has to see it.
                    let reverse = mappings_of(&emulator, arn, Some("RELATED_CONTROL")).await;
                    assert!(
                        reverse.iter().any(|mapping| {
                            let other = &mapping["Mapping"]["RelatedControl"];
                            other["ControlArn"].as_str() == Some(control.arn.as_str())
                                && other["RelationType"].as_str() == Some(relation)
                        }),
                        "case {case}: {arn} must report the {relation} relation back to it, \
                         got {reverse:?}"
                    );
                }
                "Framework" => {
                    let (min, max) = FRAMEWORK_BOUNDS;
                    for name in ["Name", "Item"] {
                        let value = details[name].as_str().unwrap_or_default();
                        assert!(
                            (min..=max).contains(&value.chars().count()),
                            "case {case}: Framework {name} {value:?} is outside the modeled \
                             length {min}..{max}"
                        );
                    }
                }
                other => panic!("case {case}: {other} is not a modeled Mapping member"),
            }
        }
    }

    for mapping_type in MAPPING_TYPES {
        let count = checked.get(mapping_type).copied().unwrap_or_default();
        assert!(
            count > 0,
            "case catalog: expected {mapping_type} targets to be checked, checked {checked:?}"
        );
    }
}
