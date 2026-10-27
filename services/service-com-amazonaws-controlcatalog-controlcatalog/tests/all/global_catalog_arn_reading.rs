// SPDX-License-Identifier: CC0-1.0

//! ONE reading of a global catalog ARN, shared by every entry point that accepts one.
//!
//! The rule under test: a global `controlcatalog` ARN — service segment
//! `controlcatalog` with an EMPTY Region segment — names the catalog resource of its
//! kind by the opaque identifier after `{kind}/`; the `{PARTITION}` segment says only
//! which partition the caller writes the ARN in, so it carries no identity, and an ARN
//! that is not of that form names no catalog resource at all.
//!
//! The point of this suite is that the rule is ONE rule: the four places a caller can
//! hand this service a catalog ARN reach it through two different code paths —
//! `GetControl` through the control resolution, and `ListObjectives`,
//! `ListCommonControls` and `ListControlMappings` through the filter canonicalisation —
//! and every row below is asserted at each of them, so a change to what counts as a
//! global catalog ARN that reached only one path would fail here.
//!
//! Each test is a TABLE: the entry points crossed with the spellings the modeled ARN
//! patterns admit (every partition of the `aws` family, a Region segment where a
//! pattern allows one, an identifier the catalog publishes under another kind, an
//! identifier it publishes nowhere), generated from the catalog the service serves.
//!
//! The harness (`Emulator`, `drain`, `error_code`) is the one `control_catalog.rs`
//! already drives the service through.

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    ComAmazonawsControlcatalogControlcatalogState as Catalog, MappingDetails,
};

use crate::control_catalog::{drain, error_code, Emulator};

/// Partitions a catalog ARN can be written in. Source: the modeled `ControlArn`,
/// `DomainArn`, `ObjectiveArn` and `CommonControlArn` patterns, each of which opens
/// `^arn:(aws(?:[-a-z]*)?):` — the whole `aws` partition family
/// (`smithy_query(query="shape", shape="ControlArn,DomainArn,ObjectiveArn,CommonControlArn")`).
const PARTITIONS: [&str; 4] = ["aws", "aws-cn", "aws-us-gov", "aws-iso-e"];

/// Region segments a caller can write where a modeled pattern admits one: the
/// `ControlArn` pattern allows `[a-zA-Z0-9-]*` there, so a Region of any case reaches
/// the handler. A GLOBAL catalog ARN carries none of them.
const REGIONS: [&str; 3] = ["us-east-1", "eu-central-1", "AP-SOUTHEAST-2"];

/// An identifier of the length the catalog's own ARNs carry that nothing is published
/// under, in the `[0-9a-z]` class every catalog ARN pattern admits.
const UNPUBLISHED_IDENTIFIER: &str = "zzzzzzzzzzzzzzzzzzzzzzzzz";

/// How many published resources of each kind a table walks. The dimension this rule
/// generalises over is the SPELLING and the entry point, not which resource is asked
/// for — every published resource of every kind is already walked by
/// `control_arn_resolution.rs` and `catalog_resource_identity.rs`.
const ROWS_PER_KIND: usize = 3;

/// Where a caller hands this service a global catalog ARN.
#[derive(Debug, Clone, Copy)]
enum EntryPoint {
    /// `GetControl`'s `ControlArn`, which resolves a control.
    GetControl,
    /// `ListObjectives`' `ObjectiveFilter.Domains` entry, which narrows to a domain.
    ObjectiveFilterDomain,
    /// `ListCommonControls`' `CommonControlFilter.Objectives` entry, which narrows to
    /// an objective.
    CommonControlFilterObjective,
    /// `ListControlMappings`' `Filter.CommonControlArns`, which narrows to a common
    /// control.
    MappingFilterCommonControl,
}

/// Every entry point, so each table states the rule once and applies it to all of them.
const ENTRY_POINTS: [EntryPoint; 4] = [
    EntryPoint::GetControl,
    EntryPoint::ObjectiveFilterDomain,
    EntryPoint::CommonControlFilterObjective,
    EntryPoint::MappingFilterCommonControl,
];

impl EntryPoint {
    /// The catalog resource kind this entry point's ARN names — the `{kind}` segment.
    fn kind(self) -> &'static str {
        match self {
            Self::GetControl => "control",
            Self::ObjectiveFilterDomain => "domain",
            Self::CommonControlFilterObjective => "objective",
            Self::MappingFilterCommonControl => "common-control",
        }
    }

    /// The route the operation is served on.
    fn route(self) -> &'static str {
        match self {
            Self::GetControl => "/get-control",
            Self::ObjectiveFilterDomain => "/objectives",
            Self::CommonControlFilterObjective => "/common-controls",
            Self::MappingFilterCommonControl => "/list-control-mappings",
        }
    }

    /// The response member an answer's identity is read from: the items list of a list
    /// operation, and `GetControl`'s own `Arn`.
    fn reported_member(self) -> &'static str {
        match self {
            Self::GetControl => "Arn",
            Self::ObjectiveFilterDomain => "Objectives",
            Self::CommonControlFilterObjective => "CommonControls",
            Self::MappingFilterCommonControl => "ControlMappings",
        }
    }

    /// The request body that hands `arn` to this entry point.
    fn body(self, arn: &str) -> String {
        match self {
            Self::GetControl => format!(r#"{{"ControlArn":"{arn}"}}"#),
            Self::ObjectiveFilterDomain => {
                format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{arn}"}}]}}}}"#)
            }
            Self::CommonControlFilterObjective => {
                format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{arn}"}}]}}}}"#)
            }
            Self::MappingFilterCommonControl => {
                format!(r#"{{"Filter":{{"CommonControlArns":["{arn}"]}}}}"#)
            }
        }
    }

    /// Whether the modeled pattern on this entry point's ARN member admits a Region
    /// segment. Only `ControlArn` does (`[a-zA-Z0-9-]*`); the three ontology patterns
    /// pin `controlcatalog:::`, so a regioned ARN never reaches their handlers.
    fn pattern_admits_a_region(self) -> bool {
        matches!(self, Self::GetControl)
    }

    /// Stored ARNs this entry point can be asked about, each of which names something:
    /// a control, a domain with objectives under it, an objective with common controls
    /// under it, or a common control some control mapping names.
    fn published(self, catalog: &Catalog) -> Vec<String> {
        let arns: Vec<String> = match self {
            Self::GetControl => catalog.controls.keys().cloned().collect(),
            Self::ObjectiveFilterDomain => catalog.domains.keys().cloned().collect(),
            Self::CommonControlFilterObjective => catalog.objectives.keys().cloned().collect(),
            Self::MappingFilterCommonControl => {
                let mut mapped: Vec<String> = catalog
                    .control_mappings
                    .values()
                    .filter_map(|record| match &record.mapping {
                        MappingDetails::CommonControl { common_control_arn } => {
                            Some(common_control_arn.clone())
                        }
                        _ => None,
                    })
                    .collect();
                mapped.sort();
                mapped.dedup();
                mapped
            }
        };
        arns.into_iter().take(ROWS_PER_KIND).collect()
    }
}

/// What an entry point reports for `arn`: the identities it names, in the order it
/// reported them, or an empty list when the ARN names nothing.
///
/// Any answer other than those two — a boundary refusal, a server error — fails here,
/// so a row can never pass by being rejected before the ARN was read.
async fn names(emulator: &Emulator, point: EntryPoint, arn: &str) -> Vec<Value> {
    match point {
        EntryPoint::GetControl => {
            let (status, response) = emulator.post(point.route(), &point.body(arn)).await;
            match status {
                200 => vec![response[point.reported_member()].clone()],
                404 => {
                    assert_eq!(
                        error_code(&response),
                        "ResourceNotFoundException",
                        "case {arn}: an ARN that names no control must be the modeled \
                         not-found, got {response:?}"
                    );
                    Vec::new()
                }
                other => panic!(
                    "case {arn}: expected 200 or 404 from {}, got {other} with {response:?}",
                    point.route()
                ),
            }
        }
        // Page size 2, so the filter and the pagination cursor that fingerprints it are
        // exercised together for every spelling.
        list => {
            drain(
                emulator,
                list.route(),
                list.reported_member(),
                Some(2),
                &list.body(arn),
            )
            .await
        }
    }
}

/// The identifier a stored catalog ARN carries — what follows `{kind}/`.
fn identifier(arn: &str) -> &str {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {arn}: a stored catalog ARN carries an identifier"))
}

/// A catalog ARN assembled from its segments, the one place this suite spells the form.
fn catalog_arn(partition: &str, region: &str, kind: &str, id: &str) -> String {
    format!("arn:{partition}:controlcatalog:{region}::{kind}/{id}")
}

/// Every entry point reads the identifier and ignores the partition: a published
/// resource's ARN rewritten into any partition of the `aws` family names exactly what
/// the stored spelling does.
#[tokio::test]
async fn the_partition_segment_carries_no_identity_at_any_entry_point() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for point in ENTRY_POINTS {
        let published = point.published(&catalog);
        assert!(
            !published.is_empty(),
            "case {}: the catalog must publish {} resources for this rule to have rows",
            point.route(),
            point.kind()
        );
        for stored in published {
            let baseline = names(&emulator, point, &stored).await;
            assert!(
                !baseline.is_empty(),
                "case {} {stored}: a published {} must name something",
                point.route(),
                point.kind()
            );
            for partition in PARTITIONS {
                let requested =
                    catalog_arn(partition, "", point.kind(), identifier(&stored));
                let reported = names(&emulator, point, &requested).await;
                assert_eq!(
                    reported,
                    baseline,
                    "case {} {requested}: expected the {} answer(s) {stored} names, got \
                     {reported:?}",
                    point.route(),
                    baseline.len()
                );
            }
        }
    }
}

/// A `controlcatalog` ARN that carries a Region segment is not the global form, so it
/// names no catalog resource — asserted at every entry point: where the modeled pattern
/// admits a Region the handler answers that the ARN names nothing, and where the pattern
/// pins `controlcatalog:::` the ARN is refused at the boundary instead.
#[tokio::test]
async fn a_region_segment_means_the_arn_names_no_catalog_resource() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for point in ENTRY_POINTS {
        for stored in point.published(&catalog) {
            for partition in PARTITIONS {
                for region in REGIONS {
                    let requested =
                        catalog_arn(partition, region, point.kind(), identifier(&stored));
                    if point.pattern_admits_a_region() {
                        let reported = names(&emulator, point, &requested).await;
                        assert!(
                            reported.is_empty(),
                            "case {} {requested}: a regioned catalog ARN names no \
                             resource, got {reported:?}",
                            point.route()
                        );
                    } else {
                        let (status, response) =
                            emulator.post(point.route(), &point.body(&requested)).await;
                        assert_eq!(
                            status, 400,
                            "case {} {requested}: the modeled pattern admits no Region \
                             segment, got {status} with {response:?}",
                            point.route()
                        );
                        assert_eq!(
                            error_code(&response),
                            "ValidationException",
                            "case {} {requested}: expected a ValidationException, got \
                             {response:?}",
                            point.route()
                        );
                    }
                }
            }
        }
    }
}

/// It is the kind and the identifier together that name a catalog resource: an
/// identifier the catalog publishes under ANOTHER kind, and one it publishes nowhere,
/// name nothing at every entry point, in every partition.
#[tokio::test]
async fn only_an_identifier_published_under_the_asked_kind_names_anything() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for point in ENTRY_POINTS {
        // The identifier of a published resource of a DIFFERENT kind, taken from the
        // next entry point in the table, plus one the catalog publishes nowhere.
        let foreign: Vec<String> = ENTRY_POINTS
            .into_iter()
            .filter(|other| other.kind() != point.kind())
            .filter_map(|other| {
                other
                    .published(&catalog)
                    .first()
                    .map(|arn| identifier(arn).to_string())
            })
            .collect();
        let mut candidates = foreign;
        candidates.push(UNPUBLISHED_IDENTIFIER.to_string());

        for id in candidates {
            for partition in PARTITIONS {
                let requested = catalog_arn(partition, "", point.kind(), &id);
                let reported = names(&emulator, point, &requested).await;
                assert!(
                    reported.is_empty(),
                    "case {} {requested}: an identifier the catalog publishes under no \
                     {} names nothing, got {reported:?}",
                    point.route(),
                    point.kind()
                );
            }
        }
    }
}

/// The namespace `GetControl` owns beside the global catalog form still resolves: a
/// `controltower` control ARN carries a Region segment by design, and names the control
/// whose Control Tower identifier it carries in any partition and any Region.
#[tokio::test]
async fn the_control_tower_namespace_still_resolves_with_its_region() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let rows: Vec<(String, String)> = catalog
        .controls
        .values()
        .filter_map(|control| {
            control
                .control_tower_identifier
                .clone()
                .map(|tower| (control.arn.clone(), tower))
        })
        .take(ROWS_PER_KIND)
        .collect();
    assert!(
        !rows.is_empty(),
        "case catalog: the catalog must publish Control Tower identifiers for this rule \
         to have rows"
    );

    for (stored, tower) in rows {
        for partition in PARTITIONS {
            for region in REGIONS {
                let requested = format!("arn:{partition}:controltower:{region}::control/{tower}");
                let reported = names(&emulator, EntryPoint::GetControl, &requested).await;
                assert_eq!(
                    reported,
                    vec![Value::String(stored.clone())],
                    "case {requested}: expected the control's catalog ARN {stored}, got \
                     {reported:?}"
                );
            }
        }
    }
}
