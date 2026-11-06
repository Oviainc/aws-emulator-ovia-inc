// SPDX-License-Identifier: CC0-1.0

//! `ListControlMappings`: what a control maps to is exactly what the catalog records
//! for it.
//!
//! The rule these tests pin is one sentence: the operation reports, for the controls a
//! filter admits, every control-mapping record the catalog holds — the common control
//! the control implements, each published compliance-framework item it satisfies and
//! each control it relates to, with the relation — and a filter that admits no control
//! reports nothing. So each test is a TABLE over the catalog the service serves (every
//! published control, every mapping kind, every ARN form) and compares the wire
//! response against the records in `crate::state`, rather than against one response
//! written out by hand: a projection that stopped reading state would fail every row.
//!
//! The harness (`Emulator`, `drain`, `error_code`) is the one `control_catalog.rs`
//! already drives the service through — the same `MultiServiceRouter` front door.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    ComAmazonawsControlcatalogControlcatalogState as Catalog, ControlMappingRecord, MappingDetails,
};

use crate::control_catalog::{drain, error_code, Emulator};

/// The route `ListControlMappings` is bound to, and the member carrying its items
/// (`POST /list-control-mappings`, `ControlMappings`).
///
/// `pub(crate)` so the sibling suites in this test target (see `tests/all.rs`) address
/// the operation through the one route rather than spelling it again.
pub(crate) const MAPPINGS_PATH: &str = "/list-control-mappings";
pub(crate) const MAPPINGS_KEY: &str = "ControlMappings";

/// Every value the modeled `MappingType` enum declares.
///
/// `pub(crate)` so the sibling suites in this test target (see `tests/all.rs`) read the
/// one list rather than keeping a second copy of it.
pub(crate) const MAPPING_TYPES: [&str; 3] = ["FRAMEWORK", "COMMON_CONTROL", "RELATED_CONTROL"];

/// Every value the modeled `ControlRelationType` enum declares.
///
/// `pub(crate)` so the sibling suites in this test target (see `tests/all.rs`) read the
/// one list rather than keeping a second copy of it.
pub(crate) const RELATION_TYPES: [&str; 3] = ["COMPLEMENTARY", "ALTERNATIVE", "MUTUALLY_EXCLUSIVE"];

/// Project a stored mapping record into the JSON the wire carries for the modeled
/// `ControlMapping`: the `Mapping` union is serialized as the single member naming the
/// mapping's kind, and `RelatedControl.ControlArn` is optional, so it appears only
/// when the record holds one.
fn wire(record: &ControlMappingRecord) -> Value {
    let mapping = match &record.mapping {
        MappingDetails::Framework { name, item } => {
            json!({"Framework": {"Name": name, "Item": item}})
        }
        MappingDetails::CommonControl { common_control_arn } => {
            json!({"CommonControl": {"CommonControlArn": common_control_arn}})
        }
        MappingDetails::RelatedControl {
            control_arn,
            relation_type,
        } => {
            let mut details = Map::new();
            if let Some(control_arn) = control_arn {
                details.insert("ControlArn".to_string(), json!(control_arn));
            }
            details.insert("RelationType".to_string(), json!(relation_type));
            json!({"RelatedControl": Value::Object(details)})
        }
    };
    json!({
        "ControlArn": record.control_arn,
        "MappingType": record.mapping_type(),
        "Mapping": mapping,
    })
}

/// Every mapping the catalog records, in the stable order the store keeps them in.
fn recorded(catalog: &Catalog) -> Vec<Value> {
    catalog.control_mappings.values().map(wire).collect()
}

/// Every control the catalog publishes reports the mapping set the catalog records for
/// it — the whole set, and only that control's share of it — whatever page size the
/// caller asks for.
#[tokio::test]
async fn every_published_control_reports_the_mapping_set_the_catalog_records() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let all = recorded(&catalog);
    assert!(
        !all.is_empty(),
        "case catalog: the catalog must record control mappings for its controls"
    );

    let mut covered = 0usize;
    // One row per published control; the page sizes walk the same filtered sequence
    // one mapping at a time, three at a time and in a single page.
    for control in catalog.controls.values() {
        let expected: Vec<Value> = catalog
            .control_mappings
            .values()
            .filter(|record| record.control_arn == control.arn)
            .map(wire)
            .collect();
        assert!(
            !expected.is_empty(),
            "case control {}: a published control must have a recorded mapping set",
            control.arn
        );
        let body = format!(r#"{{"Filter":{{"ControlArns":["{}"]}}}}"#, control.arn);
        for page_size in [None, Some(1u32), Some(3)] {
            let reported = drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, page_size, &body).await;
            assert_eq!(
                reported,
                expected,
                "case control {} maxResults={page_size:?}: expected the {} mapping(s) the \
                 catalog records, got {reported:?}",
                control.arn,
                expected.len()
            );
        }
        covered += expected.len();

        // The recorded set spans the kinds the catalog has to answer for: the control
        // implements a common control, and satisfies items in more than one published
        // compliance framework (a control is cross-walked into several frameworks, so
        // a single framework row cannot represent its coverage).
        let kinds: BTreeSet<&str> = expected
            .iter()
            .filter_map(|mapping| mapping["MappingType"].as_str())
            .collect();
        assert!(
            kinds.contains("COMMON_CONTROL"),
            "case control {}: expected a COMMON_CONTROL mapping, got kinds {kinds:?}",
            control.arn
        );
        let frameworks: BTreeSet<&str> = expected
            .iter()
            .filter_map(|mapping| mapping["Mapping"]["Framework"]["Name"].as_str())
            .collect();
        assert!(
            frameworks.len() >= 2,
            "case control {}: expected items in at least two published frameworks, got \
             {frameworks:?}",
            control.arn
        );
    }

    // The per-control filters partition the catalog's mappings: nothing is reported
    // for a control other than its own, and nothing is left unreported.
    assert_eq!(
        covered,
        all.len(),
        "case catalog: the per-control filters must account for every recorded mapping, \
         covered {covered} of {}",
        all.len()
    );
}

/// Each mapping kind the model declares round-trips through its own filter, and every
/// relation the catalog records is a modeled `ControlRelationType`.
#[tokio::test]
async fn every_mapping_kind_round_trips_through_its_own_filter() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    // One row per modelled MappingType: every kind the model declares must be recorded
    // by the catalog and must come back through its own filter, and only that kind.
    let mut covered = 0usize;
    for mapping_type in MAPPING_TYPES {
        let expected: Vec<Value> = catalog
            .control_mappings
            .values()
            .filter(|record| record.mapping_type() == mapping_type)
            .map(wire)
            .collect();
        let body = format!(r#"{{"Filter":{{"MappingTypes":["{mapping_type}"]}}}}"#);
        let reported = drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, Some(2), &body).await;
        assert_eq!(
            reported,
            expected,
            "case mapping type {mapping_type}: expected the {} mapping(s) the catalog records \
             of that kind, got {reported:?}",
            expected.len()
        );
        assert!(
            !expected.is_empty(),
            "case mapping type {mapping_type}: the catalog must record mappings of every \
             kind the model declares"
        );
        covered += expected.len();
    }
    assert_eq!(
        covered,
        catalog.control_mappings.len(),
        "case catalog: the per-kind filters must account for every recorded mapping"
    );

    // One row per common control the catalog publishes: the filter admits exactly the
    // mappings to it.
    for common_control_arn in catalog.common_controls.keys() {
        let expected: Vec<Value> = catalog
            .control_mappings
            .values()
            .filter(|record| match &record.mapping {
                MappingDetails::CommonControl {
                    common_control_arn: target,
                } => target == common_control_arn,
                _ => false,
            })
            .map(wire)
            .collect();
        let body = format!(r#"{{"Filter":{{"CommonControlArns":["{common_control_arn}"]}}}}"#);
        let reported = drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, None, &body).await;
        assert_eq!(
            reported, expected,
            "case common control {common_control_arn}: expected exactly the mappings to it, \
             got {reported:?}"
        );
    }

    // Every relation the catalog records is a modeled value, each names a control the
    // catalog publishes, and more than one relation is recorded — a catalog that only
    // ever says COMPLEMENTARY cannot express alternatives.
    let relations: BTreeSet<&str> = catalog
        .control_mappings
        .values()
        .filter_map(|record| match &record.mapping {
            MappingDetails::RelatedControl {
                control_arn,
                relation_type,
            } => {
                let target = control_arn.as_deref().unwrap_or_default();
                assert!(
                    catalog.controls.contains_key(target),
                    "case relation on {}: related control {target} is not published",
                    record.control_arn
                );
                Some(relation_type.as_str())
            }
            _ => None,
        })
        .collect();
    for relation in &relations {
        assert!(
            RELATION_TYPES.contains(relation),
            "case relation {relation}: not a modeled ControlRelationType {RELATION_TYPES:?}"
        );
    }
    assert!(
        relations.len() >= 2,
        "case catalog: expected more than one recorded relation type, got {relations:?}"
    );
}

/// A filter that admits no published control reports nothing, and an ARN outside the
/// modeled `ControlArn` shape is refused.
///
/// `ControlMappingFilter.ControlArns` is documented as "only mappings associated with
/// these controls are returned": the member NARROWS the recorded set, it does not
/// assert that the control exists. So a well-formed control ARN the catalog does not
/// publish selects no record and the operation answers 200 with an empty
/// `ControlMappings` list — it is `GetControl`, whose contract is to read one control,
/// that refuses such an ARN with its modeled `ResourceNotFoundException`. An ARN that
/// breaks `ControlArn`'s own `@length 34..2048` is refused by the boundary validator
/// before any filtering happens.
#[tokio::test]
async fn a_control_arn_the_catalog_does_not_publish_selects_no_mapping() {
    let emulator = Emulator::new();

    // The shortest ARN ControlArn's @length 34..2048 admits: the catalog ARN prefix
    // (33 characters) plus a one-character identifier.
    let shortest = "arn:aws:controlcatalog:::control/x";
    assert_eq!(
        shortest.len(),
        34,
        "case shortest ARN: ControlArn's modeled minimum is 34 characters"
    );

    // Rows differ in ARN form: the catalog form, another partition, the Control Tower
    // form, and the published lower length bound.
    let unpublished: [(&str, &str); 4] = [
        (
            "a catalog ARN this catalog does not publish",
            "arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi",
        ),
        (
            "a catalog ARN in another partition",
            "arn:aws-us-gov:controlcatalog:::control/9zzmjz4xlyw8nk3xb7qm5tfd1",
        ),
        (
            "a Control Tower ARN naming no control",
            "arn:aws:controltower:eu-west-1::control/AWS-GR_NOT_A_REAL_CONTROL",
        ),
        ("the shortest ARN ControlArn admits", shortest),
    ];
    for (case, arn) in unpublished {
        let body = format!(r#"{{"Filter":{{"ControlArns":["{arn}"]}}}}"#);
        let response = emulator.ok(MAPPINGS_PATH, &body).await;
        assert_eq!(
            response[MAPPINGS_KEY],
            json!([]),
            "case {case} ({arn}): expected an empty {MAPPINGS_KEY} list, got {response:?}"
        );
        assert!(
            response.get("NextToken").is_none(),
            "case {case} ({arn}): an empty page must carry no NextToken, got {response:?}"
        );

        // The same ARN read through GetControl is the modeled refusal, which is how a
        // caller tells "no such control" from "this control maps to nothing".
        let (status, read) = emulator
            .post("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
            .await;
        assert_eq!(
            status, 404,
            "case {case} ({arn}): expected GetControl to answer 404, got {status} with {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case} ({arn}): expected a ResourceNotFoundException, got {read:?}"
        );
    }

    // One character past each published bound of ControlArn's @length 34..2048, and a
    // filter list past the @length 1..1 ControlMappingFilter declares.
    let too_short = &shortest[..33];
    let too_long = format!("arn:aws:controlcatalog:::control/{}", "a".repeat(2016));
    assert_eq!(
        too_long.len(),
        2049,
        "case longest ARN: ControlArn's modeled maximum is 2048 characters"
    );
    let published = Catalog::default()
        .controls
        .keys()
        .next()
        .cloned()
        .expect("the catalog publishes controls");
    let refused: [(&str, String); 3] = [
        (
            "one character short of ControlArn's minimum",
            format!(r#"{{"Filter":{{"ControlArns":["{too_short}"]}}}}"#),
        ),
        (
            "one character past ControlArn's maximum",
            format!(r#"{{"Filter":{{"ControlArns":["{too_long}"]}}}}"#),
        ),
        (
            "two ARNs where the filter admits one",
            format!(r#"{{"Filter":{{"ControlArns":["{published}","{published}"]}}}}"#),
        ),
    ];
    for (case, body) in refused {
        let (status, response) = emulator.post(MAPPINGS_PATH, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: expected a ValidationException, got {response:?}"
        );
    }
}
