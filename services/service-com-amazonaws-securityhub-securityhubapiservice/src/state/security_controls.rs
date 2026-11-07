// SPDX-License-Identifier: CC0-1.0

//! The security controls this emulator publishes, and how one is reported.
//!
//! A security control is STANDARD-AGNOSTIC: the model describes
//! `SecurityControlDefinition` as "metadata for a security control, including its
//! unique standard-agnostic identifier, title, description, severity, availability in
//! Amazon Web Services Regions, and a link to remediation steps"
//! (`smithy_query(query="doc", shape="SecurityControlDefinition")`). So a control
//! appears here ONCE, under the id every standard built on the security controls
//! addresses it by, carrying the metadata that does not depend on a standard — which
//! is why this is a module of its own beside the per-standard control sets in
//! [`super::STANDARDS_CONTROL_CATALOG`]: those record what a STANDARD publishes for a
//! control, this records the control itself.
//!
//! The published metadata is service metadata, identical for every caller, so nothing
//! here is per account, per hub or per request; only the Region a request was signed
//! for decides whether a control is available in it.

use std::collections::HashMap;

use super::*;
use crate::ops::types::{
    ConfigurationOptions, IntegerConfigurationOptions, ParameterDefinition,
    SecurityControlDefinition, StringListConfigurationOptions,
};

/// `RegionAvailabilityStatus$AVAILABLE`: the control is available in the Region the
/// request was made in (`smithy_query(query="shape",
/// shape="RegionAvailabilityStatus")`). Every control of this catalog is published in
/// every Region this emulator serves — the catalog carries no Region-specific entry —
/// so a published control is available wherever it is asked for.
pub const REGION_AVAILABILITY_AVAILABLE: &str = "AVAILABLE";

/// `SecurityControlProperty$Parameters`, the only property a control's definition can
/// report as customizable (`smithy_query(query="shape",
/// shape="SecurityControlProperty")`).
pub const SECURITY_CONTROL_PROPERTY_PARAMETERS: &str = "Parameters";

/// `SecurityControlsProvider$AWS`: the control evaluates Amazon Web Services
/// resources (`smithy_query(query="shape", shape="SecurityControlsProvider")`).
pub const SECURITY_CONTROLS_PROVIDER_AWS: &str = "AWS";

/// `SecurityControlsProvider$Azure`: the control evaluates Azure resources.
pub const SECURITY_CONTROLS_PROVIDER_AZURE: &str = "Azure";

/// The options one customizable control parameter accepts, as the `ConfigurationOptions`
/// union models them (`smithy_query(query="shape", shape="ConfigurationOptions")`).
/// One variant per union member this catalog uses; a parameter of another type adds its
/// variant here rather than a second table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParameterConfiguration {
    /// `ConfigurationOptions$Integer`: a whole number, with the value the control uses
    /// until a caller customizes it and the inclusive bounds a custom value must fall in.
    Integer {
        default_value: i32,
        min: i32,
        max: i32,
    },
    /// `ConfigurationOptions$StringList`: a list of strings, bounded in length, each
    /// matching the published RE2 expression. The parameter has no default value: the
    /// control uses the list a caller sets, and reports `DefaultValue` as absent.
    StringList {
        max_items: i32,
        re2_expression: &'static str,
        expression_description: &'static str,
    },
}

/// One customizable parameter of one security control: the name the parameter is
/// addressed by, the description the control publishes for it and the options a custom
/// value must satisfy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PublishedControlParameter {
    /// The key of the `ParameterDefinitions` map entry — the name
    /// `UpdateSecurityControl` sets the parameter by.
    ///
    /// Named `PublishedControlParameter` rather than after the modelled shape because
    /// `SecurityControlParameter` is the modelled shape a CALLER sends a custom value
    /// in; this is what the service publishes a parameter's definition from.
    pub name: &'static str,
    pub description: &'static str,
    pub configuration: ParameterConfiguration,
}

/// One security control this emulator publishes: the control's own standard-agnostic
/// metadata ([`StandardsControlDefinition`], shared with every standard that addresses
/// the control by this id), the cloud provider whose resources it evaluates and the
/// parameters it can be customized with.
#[derive(Debug, Clone, Copy)]
pub struct SecurityControlCatalogEntry {
    /// The control's id, title, description and severity rating — one definition,
    /// shared with the standards that carry the control, so a control cannot be
    /// described differently by the two reads.
    pub control: StandardsControlDefinition,
    /// A `SecurityControlsProvider` value: "The cloud provider whose resources the
    /// security control evaluates" (`smithy_query(query="doc",
    /// shape="SecurityControlDefinition", member="Provider")`).
    pub provider: &'static str,
    /// The control's customizable parameters, empty for a control that supports none.
    pub parameters: &'static [PublishedControlParameter],
}

// The parameters of the customizable controls this emulator publishes, from the AWS
// Security Hub user guide's "Security Hub controls reference", which publishes each
// customizable control's parameters: the parameter's name, what it controls, its type
// and the values it accepts. A control the reference publishes no parameter for
// carries none here, which is what makes it report an empty `CustomizableProperties`
// and no `ParameterDefinitions` at all.

/// ACM.1's parameter: the renewal window the control checks a certificate's expiry
/// against — the "specified time period" of the control's title, whose published
/// default is the 30 days the control's description names.
const ACM_1_PARAMETERS: [PublishedControlParameter; 1] = [PublishedControlParameter {
    name: "daysToExpiration",
    description: "The number of days before expiration within which an ACM certificate must be renewed",
    configuration: ParameterConfiguration::Integer {
        default_value: 30,
        min: 14,
        max: 365,
    },
}];

/// The parameter of a "should be tagged" control: the tag keys the evaluated resource
/// must carry. The reference publishes the same parameter for every tagging control,
/// so the entries share this definition rather than restating it.
const REQUIRED_TAG_KEYS_PARAMETERS: [PublishedControlParameter; 1] = [PublishedControlParameter {
    name: "requiredTagKeys",
    description: "The list of tag keys that the evaluated resource must be tagged with",
    configuration: ParameterConfiguration::StringList {
        max_items: 6,
        // The characters a tag key may hold: letters, numbers, spaces and the
        // punctuation AWS tagging allows (`+ - = . _ : / @`).
        re2_expression: "^[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]+$",
        expression_description: "A tag key may hold letters, numbers, spaces and the characters + - = . _ : / @",
    },
}];

/// The security controls this emulator publishes, one entry per control.
///
/// The entries are the standard-agnostic controls the standards of
/// [`super::STANDARDS_CATALOG`] address by the control's own id; a standard that
/// predates consolidated control findings addresses its controls by its own numbered
/// requirement ids instead (the `CIS.` and `PCI.` ids of
/// [`super::STANDARDS_CONTROL_CATALOG`]), and a requirement id is not a security
/// control id, so none of those appears here.
///
/// Ordered by `SecurityControlId`, which is the order a list of definitions is
/// reported in, so a page boundary falls in the same place on every call.
pub const SECURITY_CONTROL_CATALOG: [SecurityControlCatalogEntry; 22] = [
    SecurityControlCatalogEntry {
        control: ACM_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &ACM_1_PARAMETERS,
    },
    SecurityControlCatalogEntry {
        control: AZURE_KEY_VAULT_1,
        provider: SECURITY_CONTROLS_PROVIDER_AZURE,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: AZURE_SQL_1,
        provider: SECURITY_CONTROLS_PROVIDER_AZURE,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: AZURE_STORAGE_1,
        provider: SECURITY_CONTROLS_PROVIDER_AZURE,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: AZURE_VM_1,
        provider: SECURITY_CONTROLS_PROVIDER_AZURE,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: BEDROCK_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: CLOUDTRAIL_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: CLOUDTRAIL_2,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: CLOUDWATCH_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: CONFIG_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: EC2_13,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: EC2_2,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: EC2_52,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &REQUIRED_TAG_KEYS_PARAMETERS,
    },
    SecurityControlCatalogEntry {
        control: ECS_13,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &REQUIRED_TAG_KEYS_PARAMETERS,
    },
    SecurityControlCatalogEntry {
        control: GUARDDUTY_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: IAM_4,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: KMS_4,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: S3_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: S3_5,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: SAGEMAKER_1,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: SAGEMAKER_2,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
    SecurityControlCatalogEntry {
        control: SAGEMAKER_3,
        provider: SECURITY_CONTROLS_PROVIDER_AWS,
        parameters: &[],
    },
];

/// The published security control this `SecurityControlId` names, or `None` when the
/// catalog publishes no control under it. The single definition of which ids name a
/// security control, so every operation that takes one resolves it the same way.
pub fn security_control_by_id(security_control_id: &str) -> Option<&'static SecurityControlCatalogEntry> {
    SECURITY_CONTROL_CATALOG
        .iter()
        .find(|entry| entry.control.control_id == security_control_id)
}

/// Whether `standard` applies `entry`: a standard applies a security control when the
/// control set it publishes addresses that control by the control's own id.
pub fn standard_applies_security_control(
    standard: &StandardDefinition,
    entry: &SecurityControlCatalogEntry,
) -> bool {
    standard_controls(standard)
        .iter()
        .any(|control| control.control_id == entry.control.control_id)
}

/// One customizable parameter as the modelled `ParameterDefinition` reports it.
fn parameter_definition_view(parameter: &PublishedControlParameter) -> ParameterDefinition {
    let configuration_options = match parameter.configuration {
        ParameterConfiguration::Integer {
            default_value,
            min,
            max,
        } => ConfigurationOptions::Integer(IntegerConfigurationOptions {
            default_value: Some(default_value),
            min: Some(min),
            max: Some(max),
        }),
        ParameterConfiguration::StringList {
            max_items,
            re2_expression,
            expression_description,
        } => ConfigurationOptions::StringList(StringListConfigurationOptions {
            default_value: None,
            re2_expression: Some(re2_expression.to_string()),
            max_items: Some(max_items),
            expression_description: Some(expression_description.to_string()),
        }),
    };
    ParameterDefinition {
        description: parameter.description.to_string(),
        configuration_options,
    }
}

/// One published control as the modelled `SecurityControlDefinition` every operation
/// that reports a control definition answers with, so two reads of the same control
/// cannot describe it differently.
///
/// A control that supports no custom property reports an EMPTY `CustomizableProperties`
/// ("An empty array is returned for controls that don't support custom properties")
/// and no `ParameterDefinitions` at all ("This object is excluded for a control that
/// doesn't support custom parameters") — `smithy_query(query="doc",
/// shape="SecurityControlDefinition", member="CustomizableProperties"` /
/// `"ParameterDefinitions")`.
pub fn security_control_definition_view(
    entry: &SecurityControlCatalogEntry,
) -> SecurityControlDefinition {
    let customizable = !entry.parameters.is_empty();
    SecurityControlDefinition {
        security_control_id: entry.control.control_id.to_string(),
        title: entry.control.title.to_string(),
        description: entry.control.description.to_string(),
        // A control's remediation link carries the control's own id, the same form a
        // standards control reports, so the two reads link to the same page.
        remediation_url: standards_control_remediation_url(entry.control.control_id),
        severity_rating: entry.control.severity_rating.to_string(),
        current_region_availability: REGION_AVAILABILITY_AVAILABLE.to_string(),
        customizable_properties: Some(if customizable {
            vec![SECURITY_CONTROL_PROPERTY_PARAMETERS.to_string()]
        } else {
            Vec::new()
        }),
        parameter_definitions: customizable.then(|| {
            entry
                .parameters
                .iter()
                .map(|parameter| {
                    (
                        parameter.name.to_string(),
                        parameter_definition_view(parameter),
                    )
                })
                .collect::<HashMap<String, ParameterDefinition>>()
        }),
        provider: Some(entry.provider.to_string()),
    }
}
