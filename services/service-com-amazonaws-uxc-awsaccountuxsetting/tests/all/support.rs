// SPDX-License-Identifier: CC0-1.0

//! What a suite EXPECTS a list setting to read back as, in one place.
//!
//! The harness that drives requests is `crate::wire`; this module holds only the
//! expectation helper the suites share, so none of them re-derives the rule it is
//! asserting in its own words.

use crate::wire::strings;
use serde_json::Value;

/// What a list setting reads back as after a request supplied `values`: exactly
/// that sequence.
///
/// A list setting's value is the caller's document, and the element order of a
/// JSON array is part of it: `ServiceList`/`RegionsList` declare no
/// `@uniqueItems` and no ordering, so the service neither sorts nor de-duplicates
/// what it was sent. One helper, so a suite cannot quietly assert a different
/// expectation than its neighbour.
pub fn reported(values: &[&str]) -> Value {
    strings(values)
}

/// [`reported`] for members a case built as owned strings (a generated list, or
/// one read back from `ListServices`).
pub fn reported_owned(values: &[String]) -> Value {
    reported(&values.iter().map(String::as_str).collect::<Vec<&str>>())
}
