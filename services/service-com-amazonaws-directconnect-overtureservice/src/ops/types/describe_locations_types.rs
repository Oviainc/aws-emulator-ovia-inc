// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_locations` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `DescribeLocations`
//! because its input is `smithy.api#Unit`, a construct it does not emit yet (it was
//! recorded in `src/ops/SKIPPED_STUBS.md`), so neither this module nor the handler
//! existed and the operation fell through to the framework's unroutable-operation
//! path. Everything here follows the pattern of a rendered sibling `_types` module.
//!
//! `Location` and `AvailableBillingMode` are reached by this operation alone, so
//! they are declared here rather than in `shared_types`; each structure is declared
//! exactly once across this service's `types/` modules.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-directconnect-overtureservice.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct path
/// emits the `SCHEMA`, the `DESCRIBE_LOCATIONS_INPUT_SCHEMA` static and both the
/// serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeLocationsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.directconnect#Locations")]
#[derive(Debug, Clone, Default)]
pub struct DescribeLocationsOutput {
    pub locations: Option<Vec<Location>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.directconnect#Location")]
#[derive(Debug, Clone, Default)]
pub struct Location {
    pub location_code: Option<String>,
    pub location_name: Option<String>,
    pub region: Option<String>,
    pub available_port_speeds: Option<Vec<String>>,
    pub available_providers: Option<Vec<String>>,
    pub available_mac_sec_port_speeds: Option<Vec<String>>,
    pub available_billing_modes: Option<Vec<AvailableBillingMode>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.directconnect#AvailableBillingMode")]
#[derive(Debug, Clone, Default)]
pub struct AvailableBillingMode {
    pub billing_mode: Option<String>,
    pub available_port_speeds: Option<Vec<String>>,
    pub included_regions: Option<Vec<String>>,
}
