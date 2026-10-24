// SPDX-License-Identifier: CC0-1.0

//! What a suite EXPECTS a list setting to read back as, in one place.
//!
//! The harness that drives requests is `crate::wire`; this module holds only the
//! expectation helper the suites share, so none of them re-derives the rule it is
//! asserting in its own words.

use serde_json::Value;

/// What a list setting (`visibleServices` / `visibleRegions`) reads back as after
/// a request supplied `values`: the SAME MEMBERS, in the service's one canonical
/// sequence — ascending — whatever sequence the request used.
///
/// The model fixes no order (`ServiceList`/`RegionsList` are plain lists, no
/// `@uniqueItems`, no sort trait —
/// `smithy_query(query="shape", shape="ServiceList,RegionsList")`), so this is
/// recorded behaviour: the corpus reports these settings ascending. Nothing is
/// de-duplicated, because neither list declares `@uniqueItems`, so a member named
/// twice comes back twice.
///
/// Computed here with a sort written out by hand rather than by calling the
/// emulator's own `state::canonical_members`, so a suite asserts the rule and not
/// the implementation. One helper, so a suite cannot quietly assert a different
/// expectation than its neighbour.
pub fn reported(values: &[&str]) -> Value {
    let mut members: Vec<&str> = values.to_vec();
    // Insertion sort over the supplied members, comparing the identifiers' bytes:
    // a hand-written ascending order, independent of the service's own.
    for position in 1..members.len() {
        let mut at = position;
        while at > 0 && members[at - 1].as_bytes() > members[at].as_bytes() {
            members.swap(at - 1, at);
            at -= 1;
        }
    }
    Value::Array(members.into_iter().map(Value::from).collect())
}

/// [`reported`] for members a case built as owned strings (a generated list, or
/// one read back from `ListServices`).
pub fn reported_owned(values: &[String]) -> Value {
    reported(&values.iter().map(String::as_str).collect::<Vec<&str>>())
}

/// The members of a reported list setting, for an assertion about the ARRAY rather
/// than about one expected document: panics if `body` has no such array.
pub fn members(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an array at {member} in {body}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("expected string members at {member} in {body}"))
                .to_string()
        })
        .collect()
}
