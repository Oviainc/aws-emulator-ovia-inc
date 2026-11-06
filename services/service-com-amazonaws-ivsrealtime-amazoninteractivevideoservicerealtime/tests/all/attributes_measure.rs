// SPDX-License-Identifier: CC0-1.0

//! The ONE measure the documented 1 KB `attributes` bound is enforced on, and the ONE
//! sentence a map past it is refused with, shared by every suite that drives that bound.
//!
//! The service measures the map as the request carried it — its JSON encoding — in
//! `crate::state::attributes_size`, and words the refusal in the member-constraint
//! envelope. A suite computing either for itself could agree with the bound it meant to
//! pin while disagreeing with the service, so there is one copy here and every suite that
//! drives the bound uses it.

use std::collections::BTreeMap;

/// The documented maximum size of an `attributes` map. The model states it on
/// `CreateParticipantTokenRequest$attributes`, `ParticipantTokenConfiguration$attributes`,
/// `CreateIngestConfigurationRequest$attributes` and
/// `StartParticipantReplicationRequest$attributes`, each "The maximum length of this field
/// is 1 KB total."
/// (`smithy_query(query="doc",shape="CreateParticipantTokenRequest",member="attributes")`).
pub const MAX_ATTRIBUTES_BYTES: usize = 1024;

/// How the refusal SPELLS that maximum: the model states the bound in kilobytes ("The
/// maximum length of this field is 1 KB total."), and the refusal quotes it in those words
/// rather than in the byte count it works out to.
pub const MAX_ATTRIBUTES_SIZE_LABEL: &str = "1 KB";

/// The size the bound is measured on: the bytes the map occupies as JSON — the braces, the
/// quoted keys, the colons, the commas and the quoted values — which is the form every
/// request carries one in.
pub fn json_size(map: &BTreeMap<String, String>) -> usize {
    serde_json::to_string(map)
        .expect("a map of strings serializes")
        .len()
}

/// The JSON punctuation a map of `pairs` entries carries, which the measured size counts
/// beside the keys and values themselves: the two braces, the two quotes around each key
/// and each value, the colon in every entry and the comma between entries.
///
/// Exact for keys and values that need no JSON escaping, which is what a fixture built
/// here holds; a key or value carrying a quote or a backslash encodes longer than its own
/// bytes, so size such a map with [`json_size`] instead of sizing it from a target.
pub fn json_punctuation(pairs: usize) -> usize {
    2 + pairs * 5 + pairs.saturating_sub(1)
}

/// The keys-and-values total a map of `pairs` entries needs for its JSON encoding to be
/// exactly `json_total` bytes — how a row lands a map, or a composition of maps, on the
/// bound. Carries [`json_punctuation`]'s no-escaping precondition.
///
/// Panics naming the shortfall when `json_total` cannot hold that many entries at all, so
/// a row added below the punctuation of its own shape fails readably instead of
/// underflowing.
pub fn raw_total(pairs: usize, json_total: usize) -> usize {
    let punctuation = json_punctuation(pairs);
    json_total.checked_sub(punctuation).unwrap_or_else(|| {
        panic!(
            "a {json_total}-byte JSON map cannot hold {pairs} entries: their punctuation \
             alone is {punctuation} bytes"
        )
    })
}

/// The sentence this service states the bound with, for the dotted `member` path the map
/// arrived in and the size it measured.
pub fn oversized_message(member: &str, size: usize) -> String {
    format!(
        "Request member: {member} failed to satisfy the constraints: {member} is required \
         to have a maximum size of {MAX_ATTRIBUTES_SIZE_LABEL}, found {size} bytes"
    )
}
