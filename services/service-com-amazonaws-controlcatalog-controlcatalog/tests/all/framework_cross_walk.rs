// SPDX-License-Identifier: CC0-1.0

//! `ListControlMappings`: how a FRAMEWORK mapping names a compliance framework, and how
//! much of a framework cross-walk a control reports.
//!
//! Two rules are pinned here, over the whole catalog rather than one control:
//!
//! 1. a FRAMEWORK mapping names the framework by the compact, version-bearing
//!    designator the catalog publishes it under (`NIST-CSF-v1.1`) — not by the
//!    framework's prose title — paired with that framework's own identifier for the
//!    requirement;
//! 2. the requirements a control satisfies follow from the control OBJECTIVE it
//!    implements, so every control implementing one objective reports that objective's
//!    whole cross-walk, and a framework that restates another framework's requirements
//!    reports them under the identifier it restates.
//!
//! Each test is a TABLE whose rows are the controls the catalog publishes and the
//! frameworks it cross-walks into, read back over the wire through the same
//! `MultiServiceRouter` front door `control_catalog.rs` drives.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    ComAmazonawsControlcatalogControlcatalogState as Catalog, PUBLISHED_FRAMEWORKS,
};

use crate::control_catalog::{drain, Emulator};

/// The route `ListControlMappings` is bound to and the member carrying its items.
const MAPPINGS_PATH: &str = "/list-control-mappings";
const MAPPINGS_KEY: &str = "ControlMappings";

/// `FrameworkName`'s and `FrameworkItem`'s modeled `@length` bounds
/// (`smithy_query(query="shape", shape="FrameworkMappingDetails")`).
const FRAMEWORK_NAME_BOUNDS: (usize, usize) = (3, 250);

/// The framework designators that restate another framework's requirements, with the
/// designator they restate: a `FedRAMP-r5` baseline is composed of
/// `NIST-SP-800-53-r5` controls under the same control identifier, and a `CMMC-Level2`
/// practice is an `NIST-SP-800-171-r2` requirement renamed after its family.
const NIST_SP_800_53_R5: &str = "NIST-SP-800-53-r5";
const NIST_SP_800_171_R2: &str = "NIST-SP-800-171-r2";
const FEDRAMP_R5: &str = "FedRAMP-r5";
const CMMC_LEVEL_2: &str = "CMMC-Level2";

/// The family abbreviation of each `NIST-SP-800-171-r2` section, in the order the
/// publication lists its requirement families. Written out here so the derivation the
/// service performs is checked against an independent table, not against itself.
const NIST_800_171_FAMILIES: [(&str, &str); 14] = [
    ("3.1", "AC"),
    ("3.2", "AT"),
    ("3.3", "AU"),
    ("3.4", "CM"),
    ("3.5", "IA"),
    ("3.6", "IR"),
    ("3.7", "MA"),
    ("3.8", "MP"),
    ("3.9", "PS"),
    ("3.10", "PE"),
    ("3.11", "RA"),
    ("3.12", "CA"),
    ("3.13", "SC"),
    ("3.14", "SI"),
];

/// The `CMMC-Level2` practice identifier that restates an 800-171 r2 requirement.
fn cmmc_practice(requirement: &str) -> String {
    let mut parts = requirement.split('.');
    let section = format!(
        "{}.{}",
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default()
    );
    let family = NIST_800_171_FAMILIES
        .iter()
        .find(|(candidate, _)| *candidate == section)
        .map(|(_, family)| *family)
        .unwrap_or_else(|| {
            panic!("case requirement {requirement}: no 800-171 family owns section {section}")
        });
    format!("{family}.L2-{requirement}")
}

/// Every `(framework, item)` pair a control reports, read over the wire.
async fn framework_items(emulator: &Emulator, control_arn: &str) -> BTreeSet<(String, String)> {
    let body = format!(r#"{{"Filter":{{"ControlArns":["{control_arn}"]}}}}"#);
    drain(emulator, MAPPINGS_PATH, MAPPINGS_KEY, None, &body)
        .await
        .iter()
        .filter(|mapping| mapping["MappingType"] == "FRAMEWORK")
        .map(|mapping| {
            let framework = &mapping["Mapping"]["Framework"];
            (
                framework["Name"].as_str().unwrap_or_default().to_string(),
                framework["Item"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every FRAMEWORK mapping names a published framework by its designator, and every
/// control is cross-walked into several of them.
#[tokio::test]
async fn a_framework_mapping_names_a_published_framework_by_its_designator() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    // One row per published control: whatever the control's surface or behavior, the
    // mapping set it reports has to name frameworks the catalog publishes, in the
    // designator form, and has to span more than a framework or two — a compliance
    // cross-walk that stopped at one framework would report no coverage anywhere else.
    let mut names_seen: BTreeSet<String> = BTreeSet::new();
    for control in catalog.controls.values() {
        let items = framework_items(&emulator, &control.arn).await;
        let frameworks: BTreeSet<&str> = items
            .iter()
            .map(|(framework, _)| framework.as_str())
            .collect();
        assert!(
            frameworks.len() >= 4,
            "case control {}: expected items in at least four published frameworks, got \
             {frameworks:?}",
            control.arn
        );
        for (framework, item) in &items {
            assert!(
                PUBLISHED_FRAMEWORKS.contains(&framework.as_str()),
                "case control {} framework {framework}: not a framework the catalog publishes \
                 ({PUBLISHED_FRAMEWORKS:?})",
                control.arn
            );
            // The designator is one token: an abbreviation and the version or revision
            // it is read at. A prose title (which carries spaces) is a different
            // surface's wording of the same framework and cannot be joined on.
            assert!(
                !framework.contains(char::is_whitespace),
                "case control {} framework {framework}: a designator carries no whitespace",
                control.arn
            );
            assert!(
                framework
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()
                        || ['-', '.', '_', '+'].contains(&character)),
                "case control {} framework {framework}: a designator is alphanumeric with \
                 separators",
                control.arn
            );
            let (min, max) = FRAMEWORK_NAME_BOUNDS;
            assert!(
                (min..=max).contains(&framework.chars().count()),
                "case control {} framework {framework}: outside FrameworkName's modeled \
                 length {min}..{max}",
                control.arn
            );
            assert!(
                (min..=max).contains(&item.chars().count()),
                "case control {} item {item}: outside FrameworkItem's modeled length \
                 {min}..{max}",
                control.arn
            );
            names_seen.insert(framework.clone());
        }
    }

    // Every designator the catalog publishes is reachable: a framework nothing maps to
    // would be a designator a client can never join on.
    for framework in PUBLISHED_FRAMEWORKS {
        assert!(
            names_seen.contains(*framework),
            "case framework {framework}: published but named by no control mapping, saw \
             {names_seen:?}"
        );
    }
}

/// A framework that restates another framework's requirements reports exactly the
/// restatements of the rows it is derived from.
#[tokio::test]
async fn a_restating_framework_reports_the_requirements_it_restates() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    // One row per published control, so the derivation is checked on every control's
    // own requirement set rather than on one hand-picked control.
    for control in catalog.controls.values() {
        let items = framework_items(&emulator, &control.arn).await;
        let of = |wanted: &str| -> BTreeSet<String> {
            items
                .iter()
                .filter(|(framework, _)| framework == wanted)
                .map(|(_, item)| item.clone())
                .collect()
        };

        let fedramp = of(FEDRAMP_R5);
        let nist_800_53 = of(NIST_SP_800_53_R5);
        assert!(
            !nist_800_53.is_empty(),
            "case control {}: expected {NIST_SP_800_53_R5} requirements, got {items:?}",
            control.arn
        );
        assert_eq!(
            fedramp, nist_800_53,
            "case control {}: expected {FEDRAMP_R5} to report the {NIST_SP_800_53_R5} \
             controls its baseline is composed of, got {fedramp:?}",
            control.arn
        );

        let nist_800_171 = of(NIST_SP_800_171_R2);
        let expected_cmmc: BTreeSet<String> =
            nist_800_171.iter().map(|item| cmmc_practice(item)).collect();
        assert_eq!(
            of(CMMC_LEVEL_2),
            expected_cmmc,
            "case control {}: expected {CMMC_LEVEL_2} to restate the {NIST_SP_800_171_R2} \
             requirements {nist_800_171:?}, got {:?}",
            control.arn,
            of(CMMC_LEVEL_2)
        );
    }
}

/// Controls that implement one objective report that objective's cross-walk, whichever
/// surface implements them.
#[tokio::test]
async fn controls_implementing_one_objective_share_its_cross_walk() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    // Group the published controls by the common control each implements — the
    // objective-level statement of what the control does — reading the grouping off
    // the catalog's own COMMON_CONTROL mappings.
    let mut by_objective: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for control in catalog.controls.values() {
        let body = format!(
            r#"{{"Filter":{{"ControlArns":["{}"],"MappingTypes":["COMMON_CONTROL"]}}}}"#,
            control.arn
        );
        for mapping in drain(&emulator, MAPPINGS_PATH, MAPPINGS_KEY, None, &body).await {
            if let Some(objective) = mapping["Mapping"]["CommonControl"]["CommonControlArn"].as_str()
            {
                by_objective
                    .entry(objective.to_string())
                    .or_default()
                    .push(control.arn.clone());
            }
        }
    }
    assert!(
        by_objective.values().any(|controls| controls.len() > 1),
        "case catalog: expected an objective implemented by more than one control, got \
         {by_objective:?}"
    );

    // One row per objective with more than one control: the requirements they agree on
    // are the objective's own cross-walk, which has to be the bulk of what each
    // reports — a preventive control and the detective control for the same objective
    // cannot disagree about which framework requirements the objective meets.
    for (objective, controls) in &by_objective {
        if controls.len() < 2 {
            continue;
        }
        let mut shared: Option<BTreeSet<(String, String)>> = None;
        for control_arn in controls {
            let items = framework_items(&emulator, control_arn).await;
            shared = Some(match shared {
                None => items,
                Some(shared) => shared.intersection(&items).cloned().collect(),
            });
        }
        let shared = shared.unwrap_or_default();
        let frameworks: BTreeSet<&str> = shared
            .iter()
            .map(|(framework, _)| framework.as_str())
            .collect();
        assert!(
            shared.len() >= 8 && frameworks.len() >= 5,
            "case objective {objective} ({} controls): expected the controls to share the \
             objective's cross-walk, got {} item(s) across {frameworks:?}",
            controls.len(),
            shared.len()
        );
    }
}

/// A spot check on one published control, so a cross-walk that silently emptied is
/// caught even if every assertion above were satisfied vacuously.
///
/// The control is picked by the catalog designator it is published under, and the
/// requirements asserted are the asset-inventory ones: `CM-8` ("system component
/// inventory") in 800-53 r5 and in the FedRAMP baseline that restates it, safeguard
/// `1.1` in CIS v8.0 and `A.5.9` in ISO/IEC 27001.
#[tokio::test]
async fn the_asset_inventory_control_reports_its_whole_cross_walk() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    let designator = "CONFIG.ACCOUNT.DT.1";
    let control = catalog
        .controls
        .values()
        .find(|control| control.aliases.iter().any(|alias| alias == designator))
        .unwrap_or_else(|| panic!("case {designator}: the catalog publishes this control"));

    let items = framework_items(&emulator, &control.arn).await;
    let expected = [
        (NIST_SP_800_53_R5, "CM-8"),
        (FEDRAMP_R5, "CM-8"),
        ("NIST-CSF-v1.1", "ID.AM-1"),
        ("CIS-v8.0", "1.1"),
        ("ISO-IEC-27001", "A.5.9"),
        (NIST_SP_800_171_R2, "3.4.1"),
        (CMMC_LEVEL_2, "CM.L2-3.4.1"),
    ];
    for (framework, item) in expected {
        assert!(
            items.contains(&(framework.to_string(), item.to_string())),
            "case {designator} {framework} {item}: expected the requirement in the control's \
             cross-walk, got {items:?}"
        );
    }
}
