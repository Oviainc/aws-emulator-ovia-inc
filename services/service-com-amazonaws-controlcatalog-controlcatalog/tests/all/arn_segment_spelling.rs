// SPDX-License-Identifier: CC0-1.0

//! Which SPELLINGS of an ARN's segments this service reads as naming a resource.
//!
//! The rule under test: a caller-supplied ARN names a catalog resource only when every
//! segment of it is a spelling the modeled ARN patterns admit — the partition segment a
//! member of the `aws` family those patterns restrict it to, the Region segment inside
//! the class they allow there — and an ARN carrying any other spelling of either
//! segment names nothing, because what the patterns admit is the whole of what those
//! two segments say. Both segments are dropped once read (the catalog is global, and a
//! partition says only where the caller wrote the ARN), so a spelling outside the
//! modeled class would otherwise resolve a published resource as though a modeled ARN
//! had been sent.
//!
//! Every row is asserted twice, because the modeled patterns refuse an unmodeled
//! spelling before a handler runs and so make it reachable only in-process:
//!
//! * in-process, through the state the handlers read it with — `GetControl`'s own
//!   control resolution and the filter canonicalisation every list filter goes through;
//! * over the wire, through the same front door a client uses, where an in-family
//!   spelling is served and an out-of-family one is refused.
//!
//! Each test is a TABLE: the five places a caller hands this service an ARN, crossed
//! with the spellings of the segment under test (inside the modeled class and outside
//! it, one row per way of being outside it), generated from the catalog the service
//! serves.
//!
//! The harness (`Emulator`, `drain`, `error_code`) is the one `control_catalog.rs`
//! already drives the service through.

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    canonical_catalog_arn, ComAmazonawsControlcatalogControlcatalogState as Catalog,
    MappingDetails,
};

use crate::control_catalog::{drain, error_code, Emulator};

/// Partition segments the modeled ARN patterns ADMIT. Source: the `ControlArn`,
/// `DomainArn`, `ObjectiveArn` and `CommonControlArn` patterns, each of which opens
/// `^arn:(aws(?:[-a-z]*)?):`
/// (`smithy_query(query="shape", shape="ControlArn,DomainArn,ObjectiveArn,CommonControlArn")`)
/// — the letters `aws` followed by any number of lowercase letters and hyphens. So the
/// published partitions belong to the family, and so do spellings AWS publishes no
/// partition under today (`awsfoo`, `aws-`): the family is what the patterns admit, not
/// the list of partitions that exist.
const IN_FAMILY_PARTITIONS: [&str; 6] =
    ["aws", "aws-cn", "aws-us-gov", "aws-iso-e", "awsfoo", "aws-"];

/// Partition segments the family does NOT contain, one per way of falling outside the
/// `aws(?:[-a-z]*)?` class above: a digit, an uppercase letter inside the segment, an
/// uppercase suffix, an underscore, a dot, an all-uppercase spelling, a segment that
/// does not open with `aws` at all, and the empty segment.
const OUT_OF_FAMILY_PARTITIONS: [&str; 8] = [
    "aws1", "awsX", "aws-CN", "aws_cn", "aws.cn", "AWS", "ws", "",
];

/// Region segments the `ControlArn` pattern ADMITS — it allows `[a-zA-Z0-9-]*` there,
/// letters of either case, digits and hyphens, the empty segment included.
const IN_CLASS_REGIONS: [&str; 4] = ["us-east-1", "eu-central-1", "AP-SOUTHEAST-2", ""];

/// Region segments that class does NOT contain: an underscore, a space, a dot and a
/// non-ASCII letter.
const OUT_OF_CLASS_REGIONS: [&str; 4] = ["us_east_1", "us east 1", "us.east.1", "üs-east-1"];

/// How many published resources of each kind a table walks. The dimension this rule
/// generalises over is the SPELLING and the entry point, not which resource is asked
/// for — every published resource of every kind is already walked by
/// `control_arn_resolution.rs` and `catalog_resource_identity.rs`.
const ROWS_PER_KIND: usize = 2;

/// Where a caller hands this service an ARN.
#[derive(Debug, Clone, Copy)]
enum EntryPoint {
    /// `GetControl`'s `ControlArn`, which resolves a control.
    GetControl,
    /// `ListControlMappings`' `Filter.ControlArns`, which narrows to a control.
    MappingFilterControl,
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
const ENTRY_POINTS: [EntryPoint; 5] = [
    EntryPoint::GetControl,
    EntryPoint::MappingFilterControl,
    EntryPoint::ObjectiveFilterDomain,
    EntryPoint::CommonControlFilterObjective,
    EntryPoint::MappingFilterCommonControl,
];

/// The entry points whose ARN member is a `ControlArn`, the one modeled pattern that
/// admits a Region segment — and so the only ones a Region spelling reaches.
const CONTROL_ENTRY_POINTS: [EntryPoint; 2] =
    [EntryPoint::GetControl, EntryPoint::MappingFilterControl];

impl EntryPoint {
    /// The catalog resource kind this entry point's ARN names — the `{kind}` segment.
    fn kind(self) -> &'static str {
        match self {
            Self::GetControl | Self::MappingFilterControl => "control",
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
            Self::MappingFilterControl | Self::MappingFilterCommonControl => {
                "/list-control-mappings"
            }
        }
    }

    /// The response member an answer's identity is read from: the items list of a list
    /// operation, and `GetControl`'s own `Arn`.
    fn reported_member(self) -> &'static str {
        match self {
            Self::GetControl => "Arn",
            Self::ObjectiveFilterDomain => "Objectives",
            Self::CommonControlFilterObjective => "CommonControls",
            Self::MappingFilterControl | Self::MappingFilterCommonControl => "ControlMappings",
        }
    }

    /// The request body that hands `arn` to this entry point.
    fn body(self, arn: &str) -> String {
        match self {
            Self::GetControl => format!(r#"{{"ControlArn":"{arn}"}}"#),
            Self::MappingFilterControl => format!(r#"{{"Filter":{{"ControlArns":["{arn}"]}}}}"#),
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

    /// Stored ARNs this entry point can be asked about, each of which names something:
    /// a control, a domain with objectives under it, an objective with common controls
    /// under it, or a common control some control mapping names.
    fn published(self, catalog: &Catalog) -> Vec<String> {
        let arns: Vec<String> = match self {
            Self::GetControl | Self::MappingFilterControl => {
                catalog.controls.keys().cloned().collect()
            }
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

    /// What this entry point READS `arn` as in-process: the stored ARN of the resource
    /// it names, and `arn` unchanged when it names none — the crate's one
    /// resolve-or-keep rule, through the state call this entry point's handler makes.
    fn reading(self, catalog: &Catalog, arn: &str) -> String {
        match self {
            Self::GetControl => catalog
                .resolve_control(arn)
                .map_or_else(|| arn.to_string(), |control| control.arn.clone()),
            Self::MappingFilterControl => catalog.canonical_control_arn(arn),
            filter => canonical_catalog_arn(filter.kind(), arn),
        }
    }
}

/// What an entry point reports for `arn` over the wire: the identities it names, in the
/// order it reported them. Any answer other than success fails here, so a row can never
/// pass by being refused before the ARN was read.
async fn served(emulator: &Emulator, point: EntryPoint, arn: &str) -> Vec<Value> {
    match point {
        EntryPoint::GetControl => {
            let (status, response) = emulator.post(point.route(), &point.body(arn)).await;
            assert_eq!(
                status, 200,
                "case {} {arn}: expected the control to be served, got {status} with \
                 {response:?}",
                point.route()
            );
            vec![response[point.reported_member()].clone()]
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

/// Assert the wire refuses `arn` at `point` with the modeled boundary refusal — what a
/// client gets for an ARN no modeled pattern admits.
async fn assert_refused_at_the_boundary(emulator: &Emulator, point: EntryPoint, arn: &str) {
    let (status, response) = emulator.post(point.route(), &point.body(arn)).await;
    assert_eq!(
        status,
        400,
        "case {} {arn}: an ARN no modeled pattern admits must be refused, got {status} \
         with {response:?}",
        point.route()
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {} {arn}: expected a ValidationException, got {response:?}",
        point.route()
    );
}

/// The identifier a stored catalog ARN carries — what follows `{kind}/`.
fn identifier(arn: &str) -> &str {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("case {arn}: a stored catalog ARN carries an identifier"))
}

/// A global catalog ARN assembled from its segments, the one place this suite spells
/// the form.
fn catalog_arn(partition: &str, kind: &str, id: &str) -> String {
    format!("arn:{partition}:controlcatalog:::{kind}/{id}")
}

/// A `controltower` control ARN assembled from its segments — the one accepted form
/// that carries a Region segment.
fn control_tower_arn(partition: &str, region: &str, tower_identifier: &str) -> String {
    format!("arn:{partition}:controltower:{region}::control/{tower_identifier}")
}

/// The published controls that also carry a Control Tower identifier, with the catalog
/// ARN each resolves to.
fn control_tower_rows(catalog: &Catalog) -> Vec<(String, String)> {
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
    rows
}

/// A partition the modeled family CONTAINS names the resource, at every entry point:
/// in-process the ARN is read as the stored one, and over the wire the answer is the
/// one the stored spelling gives.
#[tokio::test]
async fn a_partition_inside_the_modeled_family_names_the_resource() {
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
            let baseline = served(&emulator, point, &stored).await;
            for partition in IN_FAMILY_PARTITIONS {
                let requested = catalog_arn(partition, point.kind(), identifier(&stored));
                assert_eq!(
                    point.reading(&catalog, &requested),
                    stored,
                    "case {} {requested}: expected the stored ARN {stored}, got {}",
                    point.route(),
                    point.reading(&catalog, &requested)
                );
                let reported = served(&emulator, point, &requested).await;
                assert_eq!(
                    reported,
                    baseline,
                    "case {} {requested}: expected the answer {stored} names, got \
                     {reported:?}",
                    point.route()
                );
            }
        }
    }
}

/// A partition the modeled family does NOT contain names nothing, at every entry point:
/// in-process the ARN is read as nothing (kept exactly as sent), and over the wire the
/// modeled pattern refuses it — so an in-process caller gets the client's answer.
#[tokio::test]
async fn a_partition_outside_the_modeled_family_names_nothing() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for point in ENTRY_POINTS {
        for stored in point.published(&catalog) {
            for partition in OUT_OF_FAMILY_PARTITIONS {
                let requested = catalog_arn(partition, point.kind(), identifier(&stored));
                let reading = point.reading(&catalog, &requested);
                assert_eq!(
                    reading,
                    requested,
                    "case {} {requested}: a partition outside the modeled family names no \
                     {}, got {reading}",
                    point.route(),
                    point.kind()
                );
                assert_refused_at_the_boundary(&emulator, point, &requested).await;
            }
        }
    }
}

/// The same rule in the one other namespace an accepted control ARN can select: a
/// `controltower` ARN resolves in every partition the family contains and in none it
/// does not, whatever Region it carries.
#[tokio::test]
async fn the_partition_family_decides_a_control_tower_arn_too() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for (stored, tower) in control_tower_rows(&catalog) {
        for point in CONTROL_ENTRY_POINTS {
            for partition in IN_FAMILY_PARTITIONS {
                let requested = control_tower_arn(partition, "us-east-1", &tower);
                let reading = point.reading(&catalog, &requested);
                assert_eq!(
                    reading,
                    stored,
                    "case {} {requested}: expected the control's catalog ARN {stored}, got \
                     {reading}",
                    point.route()
                );
            }
            for partition in OUT_OF_FAMILY_PARTITIONS {
                let requested = control_tower_arn(partition, "us-east-1", &tower);
                let reading = point.reading(&catalog, &requested);
                assert_eq!(
                    reading,
                    requested,
                    "case {} {requested}: a partition outside the modeled family names no \
                     control, got {reading}",
                    point.route()
                );
                assert_refused_at_the_boundary(&emulator, point, &requested).await;
            }
        }
    }
}

/// The Region segment is read by the same rule as the partition: a `controltower` ARN
/// whose Region is inside the class the modeled pattern allows names the control, and
/// one whose Region is outside that class names nothing and is refused at the boundary.
#[tokio::test]
async fn a_region_outside_the_modeled_class_names_no_control() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for (stored, tower) in control_tower_rows(&catalog) {
        for point in CONTROL_ENTRY_POINTS {
            for region in IN_CLASS_REGIONS {
                let requested = control_tower_arn("aws", region, &tower);
                let reading = point.reading(&catalog, &requested);
                assert_eq!(
                    reading,
                    stored,
                    "case {} {requested}: a Region the pattern admits carries no identity, \
                     expected {stored}, got {reading}",
                    point.route()
                );
            }
            for region in OUT_OF_CLASS_REGIONS {
                let requested = control_tower_arn("aws", region, &tower);
                let reading = point.reading(&catalog, &requested);
                assert_eq!(
                    reading,
                    requested,
                    "case {} {requested}: a Region outside the modeled class names no \
                     control, got {reading}",
                    point.route()
                );
                assert_refused_at_the_boundary(&emulator, point, &requested).await;
            }
        }
    }
}
