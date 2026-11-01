// SPDX-License-Identifier: CC0-1.0

//! Shared fixture for the suites in this target: what the configuration type a
//! definition deploys requires of its `Parameters`, and a value for each of those
//! parameters.
//!
//! A configuration definition is accepted only when its `Parameters` provide every
//! parameter the table documented for its `Type` marks `(Required)`
//! (`crate::state::QuickSetupTypeEntry::required_parameters`). Most suites here are
//! about some OTHER rule and name only the parameters their own cases turn on, so they
//! fill the rest through [`with_required_parameters`]: the values it writes carry no
//! meaning for those suites, which is exactly why they are not written out case by
//! case. The requirement itself is under test in `tests/all/required_type_parameters.rs`,
//! which names every parameter of every case it sends and takes the values and the
//! per-type key list from here, so the two never state a different set.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    quick_setup_type_entry, PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
};

/// A value of the form the parameter's own documented description gives, for each
/// parameter a published configuration type requires: an account id for a delegated
/// administrator, a Region name for an aggregator Region, one of the three packages the
/// Distributor table names, an iCalendar string for a schedule, and so on
/// (`smithy_query(query="doc", shape="ConfigurationDefinitionInput",
/// member="Parameters")`). Nothing here is matched by the service, which stores a
/// parameter value as the caller wrote it.
///
/// A key the published catalog requires and this table does not know is a case to be
/// written, not a value to be guessed, so it panics rather than sending something
/// arbitrary.
pub fn documented_value(key: &str) -> &'static str {
    match key {
        PARAMETER_TARGET_ORGANIZATIONAL_UNITS => "ou-1",
        "CPackNames" => "Operational-Best-Practices-For-EC2",
        "DelegatedAccountId" => "111122223333",
        "ICalendarString" => "BEGIN:VCALENDAR",
        "JobFunction" => "operators",
        "PackagesToInstall" => "AWSCWAgent",
        "PatchPolicyName" => "patch-policy",
        "ReplaceExistingAggregator" => "false",
        "ResourceGroupName" => "instances",
        "SelectedAggregatorRegion" => "us-east-1",
        "SelectedPatchBaselines" => "[{\"baselineId\":\"AWS-DefaultPatchBaseline\"}]",
        "TargetAccounts" => "111122223333",
        "TargetRegions" => "us-east-1",
        "TargetTagKey" => "Environment",
        "TargetTagValue" => "production",
        other => panic!("no value is known for the parameter key {other:?}"),
    }
}

/// The string-valued entries of a JSON parameter object: the form the crate's own
/// parameter rules read a definition's map through
/// (`crate::state::ConfigurationParameters`).
pub fn held_parameters(parameters: &Value) -> BTreeMap<String, String> {
    parameters
        .as_object()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
        .collect()
}

/// Every parameter the table documented for `type_` requires of a definition holding
/// `parameters`, in the order that table states them: the keys whose requirement is
/// stated outright, plus the ones whose stated condition this very map meets.
///
/// A `type_` the catalog does not publish requires nothing, since it carries no
/// documented table: such a definition is refused for its type instead.
pub fn required_keys(type_: &str, parameters: &Value) -> Vec<&'static str> {
    let Some(entry) = quick_setup_type_entry(type_) else {
        return Vec::new();
    };
    let held = held_parameters(parameters);
    entry
        .required_parameters
        .iter()
        .filter(|required| required.applies(&held))
        .map(|required| required.key)
        .collect()
}

/// `parameters` with every parameter a definition of `type_` must provide that it does
/// not already name, each filled with its [`documented_value`].
///
/// An entry the caller wrote is never replaced, so a case still decides every parameter
/// it names — including one it deliberately writes empty.
pub fn with_required_parameters(type_: &str, parameters: Value) -> Value {
    let mut parameters: Map<String, Value> = parameters.as_object().cloned().unwrap_or_default();
    // The conditional requirements are read off the map as it stands, so a key is filled
    // only while the condition its table states holds for what the case named, and a key
    // a filled one brings with it is reached on the next pass.
    while let Some(key) = required_keys(type_, &Value::Object(parameters.clone()))
        .into_iter()
        .find(|key| !parameters.contains_key(*key))
    {
        parameters.insert(key.to_string(), json!(documented_value(key)));
    }
    Value::Object(parameters)
}
