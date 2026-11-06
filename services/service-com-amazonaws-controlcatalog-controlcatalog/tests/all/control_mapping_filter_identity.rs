// SPDX-License-Identifier: CC0-1.0

//! Every dimension of ONE `ControlMappingFilter` reads the ARN it carries by the
//! identity the catalog publishes the resource under.
//!
//! The rule under test: a catalog resource is named by its kind and the opaque
//! identifier after `{kind}/`, read in the namespace the ARN's service segment selects
//! — the `{PARTITION}` segment says only which partition the caller writes the ARN in,
//! and the `{REGION}` segment of the Control Tower form of a control ARN only where
//! the control was enabled. So BOTH ARN dimensions of a single `ControlMappingFilter`
//! obey that one rule: every accepted spelling of a resource's ARN narrows
//! `ListControlMappings` to the same mappings, including when the two dimensions are
//! set together and spelled in different partitions and different accepted forms, and
//! including the `NextToken` a filter's fingerprint issues.
//!
//! The sibling suites pin each dimension on its own — `control_arn_resolution.rs` for
//! `ControlArns`, `catalog_resource_identity.rs` for `CommonControlArns`. What these
//! tests add is the CONJUNCTION, and only that: one filter whose dimensions were read by
//! two different identity rules would answer a well-formed request with a
//! successful-but-empty page. A row that exercises one dimension alone belongs in those
//! suites, not here.
//!
//! Each test is a TABLE over the catalog the service serves (every published control
//! crossed with the common control it implements, crossed with the ARN spellings the
//! modeled patterns admit and with the mapping-type dimension), and every expectation is
//! computed from the records in `crate::state` rather than written out by hand.
//!
//! The harness (`Emulator`, `drain`) is the one `control_catalog.rs` already drives the
//! service through — the same `MultiServiceRouter` front door; the spellings
//! (`PARTITIONS`, `in_partition`) and the route and mapping-type constants are the ones
//! `catalog_resource_identity.rs` and `control_mappings.rs` already declare.

use serde_json::json;
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    ComAmazonawsControlcatalogControlcatalogState as Catalog, MappingDetails,
};

use crate::catalog_resource_identity::{in_partition, PARTITIONS};
use crate::control_catalog::{drain, Emulator};
use crate::control_mappings::{MAPPINGS_KEY, MAPPINGS_PATH, MAPPING_TYPES};

/// One `ListControlMappings` request body, with the dimensions the caller sets. Each
/// filter list is `@length 1..1`
/// (`smithy_query(query="shape",shape="ControlMappingFilter")`), so a dimension carries
/// at most one value.
fn filter_body(
    control_arn: Option<&str>,
    common_control_arn: Option<&str>,
    mapping_type: Option<&str>,
) -> String {
    let mut dimensions: Vec<String> = Vec::new();
    if let Some(arn) = control_arn {
        dimensions.push(format!(r#""ControlArns":["{arn}"]"#));
    }
    if let Some(arn) = common_control_arn {
        dimensions.push(format!(r#""CommonControlArns":["{arn}"]"#));
    }
    if let Some(mapping_type) = mapping_type {
        dimensions.push(format!(r#""MappingTypes":["{mapping_type}"]"#));
    }
    format!(r#"{{"Filter":{{{}}}}}"#, dimensions.join(","))
}

/// One pair the catalog records a mapping for: a published control, a common control it
/// implements, and the Control Tower identifier the control also carries when it has one.
struct Pair {
    control_arn: String,
    common_control_arn: String,
    control_tower_identifier: Option<String>,
}

/// Every `(control, common control)` pair the catalog records a COMMON_CONTROL mapping
/// for — the rows a filter setting both ARN dimensions has something to intersect.
fn pairs(catalog: &Catalog) -> Vec<Pair> {
    catalog
        .control_mappings
        .values()
        .filter_map(|record| match &record.mapping {
            MappingDetails::CommonControl { common_control_arn } => Some(Pair {
                control_arn: record.control_arn.clone(),
                common_control_arn: common_control_arn.clone(),
                control_tower_identifier: catalog
                    .controls
                    .get(&record.control_arn)
                    .and_then(|control| control.control_tower_identifier.clone()),
            }),
            _ => None,
        })
        .collect()
}

/// How many mappings the catalog records for `control_arn`, counting only those of
/// `mapping_type` when one is given: what a filter over those dimensions must report.
fn recorded_count(
    catalog: &Catalog,
    control_arn: Option<&str>,
    common_control_arn: Option<&str>,
    mapping_type: Option<&str>,
) -> usize {
    catalog
        .control_mappings
        .values()
        .filter(|record| {
            control_arn.is_none_or(|arn| record.control_arn == arn)
                && mapping_type.is_none_or(|kind| record.mapping_type() == kind)
                && common_control_arn.is_none_or(|arn| match &record.mapping {
                    MappingDetails::CommonControl { common_control_arn } => {
                        common_control_arn == arn
                    }
                    _ => false,
                })
        })
        .count()
}

/// The spellings of one control's ARN a filter may carry: the catalog form in every
/// partition, and — for a control that is also a Control Tower control — the Control
/// Tower form, in every partition and in a Region that is not the stored one.
fn control_spellings(pair: &Pair) -> Vec<String> {
    let mut spellings: Vec<String> = PARTITIONS
        .iter()
        .map(|partition| in_partition(&pair.control_arn, partition))
        .collect();
    if let Some(identifier) = pair.control_tower_identifier.as_deref() {
        spellings.extend(PARTITIONS.iter().map(|partition| {
            format!("arn:{partition}:controltower:ap-south-1::control/{identifier}")
        }));
    }
    spellings
}

/// A filter that sets both ARN dimensions narrows to the mappings both dimensions
/// admit, whichever partition and accepted form each ARN is spelled in — including the
/// two spelled in DIFFERENT partitions, and the control spelled in the Control Tower
/// form while the common control is spelled in another partition's catalog form.
#[tokio::test]
async fn both_arn_dimensions_of_one_filter_are_read_by_resource_identity() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let pairs = pairs(&catalog);
    assert!(
        !pairs.is_empty(),
        "case catalog: the catalog must map controls to common controls for this rule to \
         have rows"
    );

    for pair in &pairs {
        // The answer for the pair written the way the catalog stores both ARNs, checked
        // against the records the catalog holds for it.
        let baseline = drain(
            &emulator,
            MAPPINGS_PATH,
            MAPPINGS_KEY,
            None,
            &filter_body(Some(&pair.control_arn), Some(&pair.common_control_arn), None),
        )
        .await;
        let expected = recorded_count(
            &catalog,
            Some(&pair.control_arn),
            Some(&pair.common_control_arn),
            None,
        );
        assert_eq!(
            baseline.len(),
            expected,
            "case control {} with common control {}: expected the {expected} mapping(s) the \
             catalog records for the pair, got {baseline:?}",
            pair.control_arn,
            pair.common_control_arn
        );
        assert!(
            expected > 0,
            "case control {}: a recorded pair must select its own mapping",
            pair.control_arn
        );

        // Rows: every control spelling crossed with a common-control spelling that
        // rotates through the partitions independently, so the pair is written in the
        // same partition on some rows and in two different ones on others.
        for (index, control_arn) in control_spellings(pair).iter().enumerate() {
            for offset in [0, 1] {
                let partition = PARTITIONS[(index + offset) % PARTITIONS.len()];
                let common_control_arn = in_partition(&pair.common_control_arn, partition);
                let body = filter_body(Some(control_arn), Some(&common_control_arn), None);
                // Page size 1, so the pagination cursor the filter fingerprints is
                // exercised for every spelling alongside the match itself.
                let filtered = drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, Some(1), &body).await;
                assert_eq!(
                    filtered, baseline,
                    "case {body}: expected the mapping(s) {control_arn} and \
                     {common_control_arn} both admit, got {filtered:?}"
                );
            }
        }
    }
}

/// Adding the mapping-type dimension to the same filter still narrows by identity: the
/// COMMON_CONTROL kind keeps the intersection and either other modeled kind empties it,
/// in every spelling of both ARNs.
#[tokio::test]
async fn the_mapping_type_dimension_narrows_the_same_filter_in_every_spelling() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for pair in pairs(&catalog) {
        for partition in PARTITIONS {
            let control_arn = in_partition(&pair.control_arn, partition);
            let common_control_arn = in_partition(&pair.common_control_arn, partition);
            for mapping_type in MAPPING_TYPES {
                let body = filter_body(
                    Some(&control_arn),
                    Some(&common_control_arn),
                    Some(mapping_type),
                );
                let filtered = drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, None, &body).await;
                // Only a COMMON_CONTROL mapping carries a common control ARN at all, so
                // every other kind is emptied by the CommonControlArns dimension.
                let expected = recorded_count(
                    &catalog,
                    Some(&pair.control_arn),
                    Some(&pair.common_control_arn),
                    Some(mapping_type),
                );
                assert_eq!(
                    filtered.len(),
                    expected,
                    "case {body}: expected the {expected} mapping(s) the catalog records for \
                     the pair under {mapping_type}, got {filtered:?}"
                );
                assert_eq!(
                    expected > 0,
                    mapping_type == "COMMON_CONTROL",
                    "case {body}: only a COMMON_CONTROL mapping names a common control"
                );
            }
        }
    }
}

/// Identity narrows, it does not widen: a control paired with a common control it does
/// not implement selects nothing, in every spelling of both ARNs — so the rule above
/// cannot be satisfied by a dimension that is ignored.
#[tokio::test]
async fn a_pair_the_catalog_does_not_record_selects_nothing_in_any_spelling() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let pairs = pairs(&catalog);

    // Pair each control with a common control the catalog records no mapping from it to,
    // so every row below is a pair that must select nothing.
    let mismatched: Vec<(&Pair, &String)> = pairs
        .iter()
        .filter_map(|pair| {
            let other = pairs.iter().find(|candidate| {
                recorded_count(
                    &catalog,
                    Some(&pair.control_arn),
                    Some(&candidate.common_control_arn),
                    None,
                ) == 0
            })?;
            Some((pair, &other.common_control_arn))
        })
        .collect();
    assert!(
        !mismatched.is_empty(),
        "case catalog: the catalog must publish more than one mapped common control for \
         this rule to have rows"
    );

    for (pair, common_control_arn) in mismatched {
        // Rows: every control spelling, with the common control's partition rotating
        // independently so the two are written in the same partition on some rows and
        // in different ones on others.
        for (index, control_arn) in control_spellings(pair).iter().enumerate() {
            let partition = PARTITIONS[index % PARTITIONS.len()];
            let requested = in_partition(common_control_arn, partition);
            let body = filter_body(Some(control_arn), Some(&requested), None);
            let response = emulator.ok(MAPPINGS_PATH, &body).await;
            assert_eq!(
                response[MAPPINGS_KEY],
                json!([]),
                "case {body}: expected an empty {MAPPINGS_KEY} list, got {response:?}"
            );
            assert!(
                response.get("NextToken").is_none(),
                "case {body}: an empty page must carry no NextToken, got {response:?}"
            );
        }
    }
}

/// A `NextToken` `ListControlMappings` issued for a filter resumes the same sequence
/// when the filter is re-sent in another spelling: the token travels with the resources'
/// identities, not with the partition that produced it.
///
/// Rows are the `CommonControlArns` dimension — the one no sibling suite resumes a token
/// on, and the one the issue's cause lived on; `ControlArns` is left to
/// `control_arn_resolution.rs`, which already drains it in every accepted form.
#[tokio::test]
async fn a_mapping_page_token_resumes_across_the_spellings_of_its_filter() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    // (case, the filter written as the catalog stores its ARN, the spellings of that
    // same filter a caller may send instead)
    let rows: Vec<(String, String, Vec<String>)> = pairs(&catalog)
        .iter()
        .map(|pair| {
            (
                format!("common control {}", pair.common_control_arn),
                filter_body(None, Some(&pair.common_control_arn), None),
                PARTITIONS
                    .iter()
                    .map(|partition| {
                        filter_body(
                            None,
                            Some(&in_partition(&pair.common_control_arn, partition)),
                            None,
                        )
                    })
                    .collect(),
            )
        })
        .collect();

    let mut resumed_rows = 0usize;
    for (case, stored, spellings) in rows {
        let whole = drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, None, &stored).await;
        // A dimension that narrows to a single mapping leaves no page to resume.
        if whole.len() < 2 {
            continue;
        }
        let first = emulator
            .ok(&format!("{MAPPINGS_PATH}?maxResults=1"), &stored)
            .await;
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: a first page of 1 of {} carries a token, got {first:?}",
                    whole.len()
                )
            })
            .to_string();

        for body in spellings {
            let resumed = emulator
                .ok(
                    &format!("{MAPPINGS_PATH}?maxResults=1&nextToken={token}"),
                    &body,
                )
                .await;
            assert_eq!(
                resumed[MAPPINGS_KEY],
                json!([whole[1]]),
                "case {case} resumed with {body}: expected the second of the {} mapping(s) \
                 the stored spelling reports, got {resumed:?}",
                whole.len()
            );
        }
        resumed_rows += 1;
    }
    assert!(
        resumed_rows > 0,
        "case catalog: a filter must narrow to more than one mapping for this rule to have \
         rows"
    );
}

/// One dimension of the conjunction naming nothing the catalog publishes empties the
/// whole filter, in every spelling of the dimension that does name something: an
/// identifier the catalog publishes nowhere, a control's identifier read as a common
/// control's, and a published identifier in the wrong case are all read as themselves,
/// not rounded to the resource they resemble.
///
/// Each of those identifiers on a filter of ONE dimension is already a row of
/// `catalog_resource_identity.rs`; what is new here is each of them beside a dimension
/// that does resolve.
#[tokio::test]
async fn an_identifier_naming_nothing_empties_the_conjunction_in_every_spelling() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    // The opaque identifier a stored catalog ARN carries, for re-use under another kind.
    let identifier = |arn: &str| {
        arn.rsplit('/')
            .next()
            .unwrap_or_else(|| panic!("case {arn}: a stored catalog ARN carries an identifier"))
            .to_string()
    };
    // An identifier of the length the catalog's own ARNs carry that nothing is published
    // under, in the `[0-9a-z]` class the modeled ARN patterns admit.
    let unpublished = "9999999999999999999999999".to_string();

    for pair in pairs(&catalog) {
        // (case, the kind the unresolvable identifier is written under, that identifier)
        // — the filter's OTHER dimension carries the pair's own resource, so the row can
        // only come back empty because this identifier names nothing.
        let rows: [(&str, &str, String); 5] = [
            (
                "a control identifier the catalog publishes nowhere",
                "control",
                unpublished.clone(),
            ),
            (
                "a common control identifier the catalog publishes nowhere",
                "common-control",
                unpublished.clone(),
            ),
            (
                "the common control's identifier read as a control's",
                "control",
                identifier(&pair.common_control_arn),
            ),
            (
                "the control's identifier read as a common control's",
                "common-control",
                identifier(&pair.control_arn),
            ),
            (
                // Only the control dimension can be asked this: a common control ARN's
                // modeled pattern admits `[0-9a-z]+`, so an upper-case identifier is
                // refused at the boundary there rather than reaching the filter.
                "the control's own identifier in the wrong case",
                "control",
                identifier(&pair.control_arn).to_uppercase(),
            ),
        ];

        for (case, kind, id) in rows {
            for partition in PARTITIONS {
                // Each ARN is written in the same partition, which is not the stored one
                // on most rows, so the row exercises the identity rule on both
                // dimensions at once.
                let unresolvable = format!("arn:{partition}:controlcatalog:::{kind}/{id}");
                let body = if kind == "control" {
                    filter_body(
                        Some(&unresolvable),
                        Some(&in_partition(&pair.common_control_arn, partition)),
                        None,
                    )
                } else {
                    filter_body(
                        Some(&in_partition(&pair.control_arn, partition)),
                        Some(&unresolvable),
                        None,
                    )
                };
                let response = emulator.ok(MAPPINGS_PATH, &body).await;
                assert_eq!(
                    response[MAPPINGS_KEY],
                    json!([]),
                    "case {case} ({body}): expected an empty {MAPPINGS_KEY} list, got \
                     {response:?}"
                );
            }
        }
    }
}
