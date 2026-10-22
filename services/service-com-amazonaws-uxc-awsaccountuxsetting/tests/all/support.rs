// SPDX-License-Identifier: CC0-1.0

//! What a suite EXPECTS a list setting to read back as, in one place.
//!
//! The harness that drives requests is `crate::wire`; this module holds only the
//! expectation helper two suites share, so neither re-derives the rule it is
//! asserting in its own words.

use crate::wire::strings;
use serde_json::Value;

/// The order a list setting is reported in: the members a request named,
/// lexicographically ascending — see `state::canonical_members`.
///
/// The ORDER a caller lists members in carries no meaning to the service, so a
/// response is a function of the members, not of the sequence. A duplicate is
/// kept: `ServiceList`/`RegionsList` are lists with no `@uniqueItems`. One
/// helper, so a suite cannot quietly assert a different order than its
/// neighbour.
pub fn canonical(values: &[&str]) -> Value {
    let mut members: Vec<&str> = values.to_vec();
    members.sort_unstable();
    strings(&members)
}

/// [`canonical`] for members a case built as owned strings (a generated list, or
/// one read back from `ListServices`).
pub fn canonical_owned(values: &[String]) -> Value {
    canonical(&values.iter().map(String::as_str).collect::<Vec<&str>>())
}
