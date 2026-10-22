// SPDX-License-Identifier: CC0-1.0

//! How a control ARN names a control, and what the catalog answers for one it publishes.
//!
//! The rule under test: a control ARN names a control by the identifier after
//! `control/`, read in the namespace its service segment selects — the two forms
//! GetControl documents are `arn:{PARTITION}:controlcatalog:::control/{CONTROL_CATALOG_OPAQUE_ID}`
//! and `arn:{PARTITION}:controltower:{REGION}::control/{CONTROL_TOWER_OPAQUE_ID}`, whose
//! `{PARTITION}` and `{REGION}` segments say which partition the caller writes the ARN in
//! and where the control was enabled, not which control it is. So an ARN of either form
//! whose identifier the catalog publishes resolves with HTTP 200 and the whole published
//! record (reported under the control's own `controlcatalog` ARN), wherever a control ARN
//! is accepted, and an identifier the catalog does not publish is the modeled
//! `ResourceNotFoundException` (404) in every partition.
//!
//! Every test is a TABLE whose rows are the catalog the service serves crossed with the
//! ARN forms the model admits, rather than one ARN picked by hand.

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;

use crate::control_catalog::{drain, error_code, Emulator};

/// Partitions a control ARN can be written in. Source: GetControl's `ControlArn`
/// documentation, whose two accepted forms both open `arn:{PARTITION}:`, and the modeled
/// `ControlArn` pattern `^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):…`, which
/// admits the whole `aws` partition family.
const PARTITIONS: [&str; 4] = ["aws", "aws-us-gov", "aws-cn", "aws-iso-b"];

/// Regions the `controltower` form can name — its `{REGION}` segment. The modeled
/// `ControlArn` pattern allows `[a-zA-Z0-9-]*` there.
const CONTROL_TOWER_REGIONS: [&str; 3] = ["us-east-1", "eu-central-1", "ap-southeast-2"];

/// The members `GetControlResponse` makes required: `smithy_query(query="operation",
/// operation="GetControl")` marks exactly these five.
const REQUIRED_MEMBERS: [&str; 5] = [
    "Arn",
    "Name",
    "Description",
    "Behavior",
    "RegionConfiguration",
];

/// The opening of every stored catalog ARN: the global form, with neither a Region nor
/// an account id (the modeled `ControlArn` pattern's `controlcatalog` alternative).
const CATALOG_CONTROL_PREFIX: &str = "arn:aws:controlcatalog:::control/";

/// The opaque catalog identifier a stored control ARN carries.
fn catalog_identifier(arn: &str) -> &str {
    arn.strip_prefix(CATALOG_CONTROL_PREFIX)
        .unwrap_or_else(|| panic!("case {arn}: a stored control ARN is written in the global form"))
}

/// The strings of a JSON array member, or an empty list when the member is absent.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Every control the catalog publishes resolves from its catalog ARN written in any
/// partition, and answers with the whole stored record under the control's own ARN.
#[tokio::test]
async fn every_published_control_resolves_from_its_catalog_arn_in_any_partition() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    assert!(
        !catalog.controls.is_empty(),
        "the catalog must publish controls for this rule to have rows"
    );

    for control in catalog.controls.values() {
        let identifier = catalog_identifier(&control.arn);
        for partition in PARTITIONS {
            let requested = format!("arn:{partition}:controlcatalog:::control/{identifier}");
            let case = requested.clone();
            let response = emulator
                .ok("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
                .await;

            for member in REQUIRED_MEMBERS {
                assert!(
                    response.get(member).is_some_and(|value| !value.is_null()),
                    "case {case} member {member}: GetControlResponse requires it, got \
                     {response:?}"
                );
            }

            assert_eq!(
                response["Arn"].as_str(),
                Some(control.arn.as_str()),
                "case {case}: GetControl must answer under the control's controlcatalog \
                 ARN, got {response:?}"
            );
            assert_eq!(
                response["Name"].as_str(),
                Some(control.name.as_str()),
                "case {case}: expected the stored Name, got {response:?}"
            );
            assert_eq!(
                response["Description"].as_str(),
                Some(control.description.as_str()),
                "case {case}: expected the stored Description, got {response:?}"
            );
            assert_eq!(
                response["Behavior"].as_str(),
                Some(control.behavior.as_str()),
                "case {case}: expected the stored Behavior, got {response:?}"
            );
            assert_eq!(
                response["Severity"].as_str(),
                control.severity.as_deref(),
                "case {case}: expected the stored Severity, got {response:?}"
            );
            assert_eq!(
                response["ParameterRequirementSummary"].as_str(),
                control.parameter_requirement_summary.as_deref(),
                "case {case}: expected the stored ParameterRequirementSummary, got \
                 {response:?}"
            );
            assert_eq!(
                response["RegionConfiguration"]["Scope"].as_str(),
                Some(control.region_configuration.scope.as_str()),
                "case {case}: expected the stored Scope, got {response:?}"
            );
            assert_eq!(
                strings(&response["Aliases"]),
                control.aliases,
                "case {case}: expected the stored Aliases, got {response:?}"
            );
            assert_eq!(
                response["Implementation"]["Type"].as_str(),
                control
                    .implementation
                    .as_ref()
                    .map(|implementation| implementation.implementation_type.as_str()),
                "case {case}: expected the stored Implementation.Type, got {response:?}"
            );
            assert_eq!(
                response["Implementation"]["Identifier"].as_str(),
                control
                    .implementation
                    .as_ref()
                    .and_then(|implementation| implementation.identifier.as_deref()),
                "case {case}: expected the stored Implementation.Identifier, got \
                 {response:?}"
            );
            assert_eq!(
                strings(&response["GovernedResources"]),
                control.governed_resources,
                "case {case}: expected the stored GovernedResources, got {response:?}"
            );
            assert_eq!(
                strings(&response["GovernedProviders"]),
                control.governed_providers,
                "case {case}: expected the stored GovernedProviders, got {response:?}"
            );
            let parameters: Vec<String> = response["Parameters"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item["Name"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                parameters,
                control
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>(),
                "case {case}: expected the stored Parameters, got {response:?}"
            );
            let create_time = response["CreateTime"].as_f64().unwrap_or_else(|| {
                panic!(
                    "case {case}: CreateTime must be a timestamp, got {:?}",
                    response["CreateTime"]
                )
            });
            assert_eq!(
                create_time as i64, control.create_time,
                "case {case}: expected the stored CreateTime, got {create_time}"
            );
        }
    }
}

/// A control that is also a Control Tower control resolves from the Control Tower ARN
/// form in any partition and any Region, and still answers under its catalog ARN.
#[tokio::test]
async fn a_control_tower_arn_resolves_in_any_partition_and_region() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let rows: Vec<(String, String)> = catalog
        .controls
        .values()
        .filter_map(|control| {
            control
                .control_tower_identifier
                .clone()
                .map(|identifier| (control.arn.clone(), identifier))
        })
        .collect();
    assert!(
        rows.len() >= 2,
        "the catalog must publish Control Tower identifiers for this rule to have rows"
    );

    for (catalog_arn, identifier) in rows {
        for partition in PARTITIONS {
            for region in CONTROL_TOWER_REGIONS {
                let requested =
                    format!("arn:{partition}:controltower:{region}::control/{identifier}");
                let response = emulator
                    .ok("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
                    .await;
                assert_eq!(
                    response["Arn"].as_str(),
                    Some(catalog_arn.as_str()),
                    "case {requested}: expected the control's controlcatalog ARN, got \
                     {response:?}"
                );
            }
        }
    }
}

/// An identifier the catalog does not publish is the modeled `ResourceNotFoundException`
/// in every partition and either ARN form — including an identifier that differs from a
/// published one only in case, because an ARN names its control exactly.
#[tokio::test]
async fn an_unpublished_identifier_is_not_found_in_every_partition() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let published = catalog
        .controls
        .values()
        .next()
        .expect("the catalog publishes controls");
    let shouting = catalog_identifier(&published.arn).to_uppercase();

    // (case, service segment, Region segment, identifier)
    let rows: [(&str, &str, &str, String); 4] = [
        (
            "a catalog identifier the catalog does not publish",
            "controlcatalog",
            "",
            "zzzzzzzzzzzzzzzzzzzzzzzzz".to_string(),
        ),
        (
            "a published catalog identifier in the wrong case",
            "controlcatalog",
            "",
            shouting,
        ),
        (
            "a Control Tower identifier no control carries",
            "controltower",
            "us-east-1",
            "AWS-GR_NO_SUCH_CONTROL".to_string(),
        ),
        (
            "a catalog identifier asked for in the Control Tower namespace",
            "controltower",
            "us-east-1",
            catalog_identifier(&published.arn).to_string(),
        ),
    ];

    for (case, service, region, identifier) in rows {
        for partition in PARTITIONS {
            let requested = format!("arn:{partition}:{service}:{region}::control/{identifier}");
            let (status, response) = emulator
                .post("/get-control", &format!(r#"{{"ControlArn":"{requested}"}}"#))
                .await;
            assert_eq!(
                status, 404,
                "case {case} ({requested}): expected 404, got {status} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case} ({requested}): expected ResourceNotFoundException, got \
                 {response:?}"
            );
        }
    }
}

/// The same rule holds wherever a control ARN is accepted: `ListControlMappings`'
/// `ControlArns` filter resolves a control from either ARN form, in any partition, to the
/// same mappings.
#[tokio::test]
async fn the_control_arns_filter_resolves_a_control_from_any_accepted_arn() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let mut mappings_compared = 0usize;

    for control in catalog.controls.values() {
        let baseline = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            None,
            &format!(r#"{{"Filter":{{"ControlArns":["{}"]}}}}"#, control.arn),
        )
        .await;

        let identifier = catalog_identifier(&control.arn);
        let mut forms: Vec<String> = PARTITIONS
            .iter()
            .map(|partition| format!("arn:{partition}:controlcatalog:::control/{identifier}"))
            .collect();
        if let Some(tower_identifier) = control.control_tower_identifier.as_deref() {
            forms.extend(PARTITIONS.iter().map(|partition| {
                format!("arn:{partition}:controltower:eu-west-1::control/{tower_identifier}")
            }));
        }

        for requested in forms {
            let filtered = drain(
                &emulator,
                "/list-control-mappings",
                "ControlMappings",
                Some(2),
                &format!(r#"{{"Filter":{{"ControlArns":["{requested}"]}}}}"#),
            )
            .await;
            assert_eq!(
                filtered, baseline,
                "case {requested}: the filter must resolve the same control as \
                 {}, got {filtered:?}",
                control.arn
            );
        }
        mappings_compared += baseline.len();
    }

    assert!(
        mappings_compared > 0,
        "the catalog must hold control mappings for this rule to narrow anything"
    );
}
