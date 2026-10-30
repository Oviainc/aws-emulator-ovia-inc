// SPDX-License-Identifier: CC0-1.0

//! ONE resolve-or-keep reading of a caller-supplied ARN, shared by every filter
//! dimension in the crate that accepts one.
//!
//! The rule under test: a filter ARN is read as the ARN the catalog stores for the
//! resource that ARN names, and an ARN that names nothing the catalog publishes is kept
//! exactly as sent, so it goes on matching nothing. Both outcomes of that one rule are
//! asserted at each dimension a caller can hand an ARN to — `ListControlMappings`'
//! `Filter.ControlArns`, which resolves a control (and so also accepts the Control Tower
//! form of its ARN), beside `Filter.CommonControlArns`, `ListObjectives`'
//! `ObjectiveFilter.Domains` and `ListCommonControls`' `CommonControlFilter.Objectives`,
//! which resolve a global catalog ARN.
//!
//! The point of this suite is that those dimensions are read by the SAME rule rather
//! than by one copy of it per dimension: the control dimension used to spell the rule
//! out inline instead of calling the shared helper, so a change to the fallback that
//! reached the helper alone would have left that dimension behind. Every row below is
//! driven at all four dimensions, so such a split fails here.
//!
//! Each test is a TABLE: the dimensions crossed with the spellings the modeled ARN
//! patterns admit (every partition of the `aws` family, and the Control Tower form where
//! a control is named) and, for the keep-as-sent branch, with the identifiers that name
//! nothing (one the catalog publishes nowhere, one it publishes under another kind) —
//! generated from the catalog the service serves rather than written out by hand.
//!
//! The harness (`Emulator`, `drain`) is the one `control_catalog.rs` already drives the
//! service through; the spellings (`PARTITIONS`, `in_partition`) are the ones
//! `catalog_resource_identity.rs` declares.

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    ComAmazonawsControlcatalogControlcatalogState as Catalog, MappingDetails,
};

use crate::catalog_resource_identity::{in_partition, PARTITIONS};
use crate::control_catalog::{drain, Emulator};

/// An identifier of the length the catalog's own ARNs carry that nothing is published
/// under, in the `[0-9a-z]` class every modeled catalog ARN pattern admits.
const UNPUBLISHED_IDENTIFIER: &str = "7777777777777777777777777";

/// A Region a Control Tower control ARN can be written in: that form's modeled pattern
/// is the only one of this service's ARN patterns with a Region segment, and which
/// Region a control was enabled in is not part of its identity.
const CONTROL_TOWER_REGION: &str = "eu-west-3";

/// How many published resources each dimension is driven on. The dimension this rule
/// generalises over is the outcome of the reading and the spelling asked for, not which
/// resource is asked about.
const ROWS_PER_DIMENSION: usize = 2;

/// A filter dimension that accepts an ARN.
#[derive(Debug, Clone, Copy)]
enum Dimension {
    /// `ListControlMappings`' `Filter.ControlArns` — the control dimension, resolved
    /// through the control resolution, which also accepts the Control Tower ARN form.
    MappingControl,
    /// `ListControlMappings`' `Filter.CommonControlArns`.
    MappingCommonControl,
    /// `ListObjectives`' `ObjectiveFilter.Domains` entry.
    ObjectiveDomain,
    /// `ListCommonControls`' `CommonControlFilter.Objectives` entry.
    CommonControlObjective,
}

/// Every dimension, so each table states the rule once and applies it to all of them.
const DIMENSIONS: [Dimension; 4] = [
    Dimension::MappingControl,
    Dimension::MappingCommonControl,
    Dimension::ObjectiveDomain,
    Dimension::CommonControlObjective,
];

impl Dimension {
    /// The catalog resource kind this dimension's ARN names — the `{kind}` segment.
    fn kind(self) -> &'static str {
        match self {
            Self::MappingControl => "control",
            Self::MappingCommonControl => "common-control",
            Self::ObjectiveDomain => "domain",
            Self::CommonControlObjective => "objective",
        }
    }

    /// The route the operation carrying this dimension is served on, and the response
    /// member holding its items.
    fn route(self) -> (&'static str, &'static str) {
        match self {
            Self::MappingControl | Self::MappingCommonControl => {
                ("/list-control-mappings", "ControlMappings")
            }
            Self::ObjectiveDomain => ("/objectives", "Objectives"),
            Self::CommonControlObjective => ("/common-controls", "CommonControls"),
        }
    }

    /// The request body that sets this dimension to `arn` and no other.
    fn body(self, arn: &str) -> String {
        match self {
            Self::MappingControl => format!(r#"{{"Filter":{{"ControlArns":["{arn}"]}}}}"#),
            Self::MappingCommonControl => {
                format!(r#"{{"Filter":{{"CommonControlArns":["{arn}"]}}}}"#)
            }
            Self::ObjectiveDomain => {
                format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{arn}"}}]}}}}"#)
            }
            Self::CommonControlObjective => {
                format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{arn}"}}]}}}}"#)
            }
        }
    }

    /// Stored ARNs this dimension narrows to something with: a control the catalog
    /// records a mapping for, a common control some mapping names, a domain some
    /// objective belongs to, an objective some common control belongs to.
    fn published(self, catalog: &Catalog) -> Vec<String> {
        let mut arns: Vec<String> = match self {
            Self::MappingControl => catalog
                .control_mappings
                .values()
                .map(|record| record.control_arn.clone())
                .collect(),
            Self::MappingCommonControl => catalog
                .control_mappings
                .values()
                .filter_map(|record| match &record.mapping {
                    MappingDetails::CommonControl { common_control_arn } => {
                        Some(common_control_arn.clone())
                    }
                    _ => None,
                })
                .collect(),
            Self::ObjectiveDomain => catalog
                .objectives
                .values()
                .map(|objective| objective.domain_arn.clone())
                .collect(),
            Self::CommonControlObjective => catalog
                .common_controls
                .values()
                .map(|common_control| common_control.objective_arn.clone())
                .collect(),
        };
        arns.sort();
        arns.dedup();
        arns.into_iter().take(ROWS_PER_DIMENSION).collect()
    }

    /// Every spelling of `stored` this dimension must read as the stored ARN itself:
    /// each partition of the `aws` family, plus — for the control dimension, on a
    /// control the catalog also publishes a Control Tower identifier for — that ARN
    /// form, in each partition and in a Region that is not the control's.
    fn spellings(self, catalog: &Catalog, stored: &str) -> Vec<String> {
        let mut spellings: Vec<String> = PARTITIONS
            .iter()
            .map(|partition| in_partition(stored, partition))
            .collect();
        if matches!(self, Self::MappingControl) {
            if let Some(identifier) = catalog
                .controls
                .get(stored)
                .and_then(|control| control.control_tower_identifier.clone())
            {
                spellings.extend(PARTITIONS.iter().map(|partition| {
                    format!(
                        "arn:{partition}:controltower:{CONTROL_TOWER_REGION}::control/{identifier}"
                    )
                }));
            }
        }
        spellings
    }
}

/// The identifier a stored catalog ARN carries — what follows `{kind}/`.
fn identifier(arn: &str) -> &str {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {arn}: a stored catalog ARN carries an identifier"))
}

/// The resolve half of the rule, at every dimension: a published resource's ARN written
/// in any accepted spelling narrows the list to exactly what the ARN the catalog stores
/// narrows it to — asserted a page at a time, so the `NextToken` the filter's
/// fingerprint issues is read by the same reading as the match.
#[tokio::test]
async fn every_filter_dimension_resolves_a_published_arn_in_any_spelling() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for dimension in DIMENSIONS {
        let (route, items_key) = dimension.route();
        let published = dimension.published(&catalog);
        assert!(
            !published.is_empty(),
            "case {route} {}: the catalog must publish resources of that kind for this \
             rule to have rows",
            dimension.kind()
        );

        for stored in published {
            let baseline = drain(&emulator, route, items_key, None, &dimension.body(&stored)).await;
            assert!(
                !baseline.is_empty(),
                "case {route} {stored}: a published {} must narrow to something, got an \
                 empty {items_key} list",
                dimension.kind()
            );

            let spellings = dimension.spellings(&catalog, &stored);
            assert!(
                spellings.len() >= PARTITIONS.len(),
                "case {route} {stored}: expected at least {} spellings, got {spellings:?}",
                PARTITIONS.len()
            );
            for requested in spellings {
                let body = dimension.body(&requested);
                // Page size 1, so every spelling drives the pagination cursor the
                // filter's fingerprint keys as well as the match itself.
                let reported = drain(&emulator, route, items_key, Some(1), &body).await;
                assert_eq!(
                    reported, baseline,
                    "case {body}: expected the {} item(s) {stored} narrows to, got \
                     {reported:?}",
                    baseline.len()
                );
            }
        }
    }
}

/// The keep-as-sent half of the rule, at every dimension: an ARN that names nothing the
/// catalog publishes is read as itself, so it matches nothing — in every partition, both
/// for an identifier the catalog publishes nowhere and for one it publishes under
/// another kind.
#[tokio::test]
async fn every_filter_dimension_keeps_an_arn_naming_nothing_as_sent() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for dimension in DIMENSIONS {
        let (route, items_key) = dimension.route();
        // (case, the identifier to write under this dimension's kind)
        let mut unresolvable: Vec<(&str, String)> = vec![(
            "an identifier the catalog publishes nowhere",
            UNPUBLISHED_IDENTIFIER.to_string(),
        )];
        unresolvable.extend(
            DIMENSIONS
                .into_iter()
                .filter(|other| other.kind() != dimension.kind())
                .filter_map(|other| {
                    other.published(&catalog).first().map(|arn| {
                        (
                            "an identifier published under another kind",
                            identifier(arn).to_string(),
                        )
                    })
                }),
        );

        for (case, id) in unresolvable {
            for partition in PARTITIONS {
                let requested =
                    format!("arn:{partition}:controlcatalog:::{}/{id}", dimension.kind());
                let body = dimension.body(&requested);
                let response = emulator.ok(route, &body).await;
                assert_eq!(
                    response[items_key],
                    Value::Array(Vec::new()),
                    "case {case} ({body}): expected an empty {items_key} list, got \
                     {response:?}"
                );
                assert!(
                    response.get("NextToken").is_none(),
                    "case {case} ({body}): an empty page carries no NextToken, got \
                     {response:?}"
                );
            }
        }
    }
}
