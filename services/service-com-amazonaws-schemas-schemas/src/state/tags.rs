// SPDX-License-Identifier: CC0-1.0

//! The bounds the tag set of one resource is kept within, and the refusals past them.
//!
//! A tag set is the one thing a caller writes into this store whose size the model
//! bounds nowhere: `Tags` is a map of plain `__string` to plain `__string` with no
//! length, pattern or count constraint on either
//! (`smithy_query(query="shape",shape="Tags")`), so the framework's modelled-constraint
//! pass refuses nothing here. Everything else a caller can accumulate has an explicit
//! ceiling ([`super::MAX_REGISTRIES`] and the budgets beside it) for the same reason a
//! tag set needs one: a generated emulator keeps its resources in memory for the life
//! of the process, and `ListTagsForResource` must still answer what a tag write stored,
//! so neither a TTL nor an eviction policy is available to it.
//!
//! The numbers are AWS's published tagging limits for a resource — at most 50 tags, a
//! key of at most 128 characters and a value of at most 256 — counted in characters
//! rather than bytes, as AWS counts them. Every operation that WRITES a tag set checks
//! the same bounds through the helpers here, so the creates that seed one and the call
//! that adds to one cannot disagree about what a resource may hold.

use super::*;

/// The most tags one resource may hold.
///
/// Source: AWS's tagging limits for a resource (maximum 50 tags per resource).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most characters a tag KEY may carry.
///
/// Source: AWS's tagging limits for a resource (key length 128 Unicode characters).
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The most characters a tag VALUE may carry.
///
/// Source: AWS's tagging limits for a resource (value length 256 Unicode characters).
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Check the `Tags` map one request carries, on the request alone.
///
/// The form half of the bound, so it is decided before the store is reached at all: a
/// request naming more pairs than a resource may hold, or carrying a key or a value
/// past its length, is refused rather than stored. `None` is a request that carries no
/// `Tags` member, which asks for nothing and so breaks nothing.
///
/// `Err` is the refusal to answer, in the service's own parameter-validation voice
/// ([`super::NameConstraint::validate`]): it names what was checked and the rule that
/// was broken, never the value sent, which the caller already holds. One rule per
/// refusal, and the longest key or value decides its own rule, so the sentence does not
/// depend on the order a map happens to iterate in.
pub fn validate_tags(tags: Option<&HashMap<String, String>>) -> Result<(), BadRequestException> {
    let Some(tags) = tags else {
        return Ok(());
    };
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(request_tags_cap_reached(tags.len()));
    }
    let longest_key = tags.keys().map(|key| key.chars().count()).max();
    if longest_key.is_some_and(|longest| longest > MAX_TAG_KEY_CHARS) {
        return Err(bad_request(format!(
            "Parameter Tags must have keys of length less than or equal to {MAX_TAG_KEY_CHARS}"
        )));
    }
    let longest_value = tags.values().map(|value| value.chars().count()).max();
    if longest_value.is_some_and(|longest| longest > MAX_TAG_VALUE_CHARS) {
        return Err(bad_request(format!(
            "Parameter Tags must have values of length less than or equal to \
             {MAX_TAG_VALUE_CHARS}"
        )));
    }
    Ok(())
}

/// How many tags a resource that holds `held` would hold once `applied` is applied to
/// it: the keys it holds, plus the keys the request names that it does not already
/// hold.
///
/// A tag write ADDS a key and REPLACES the value of one already held, so only a key
/// that is new grows the set. This is the one place that is counted, so the bound a
/// write is refused past is the number the write would actually leave.
pub fn tag_count_after(held: &BTreeMap<String, String>, applied: &HashMap<String, String>) -> usize {
    held.len()
        + applied
            .keys()
            .filter(|key| !held.contains_key(key.as_str()))
            .count()
}

/// Refuse a request that NAMES more tags than one resource may hold, before the
/// store is consulted. The number it names is the request's own pair count — what the
/// resource would end up holding is [`tags_cap_reached`]'s sentence, which is the
/// refusal that can see the stored set.
pub fn request_tags_cap_reached(named: usize) -> BadRequestException {
    bad_request(format!(
        "A resource holds at most {MAX_TAGS_PER_RESOURCE} tags, and this request names \
         {named}."
    ))
}

/// Refuse a tag write that would take one resource past the number of tags it may
/// hold. Names the cap and what the write would leave, so the caller knows a retry
/// cannot help.
pub fn tags_cap_reached(would_hold: usize) -> BadRequestException {
    bad_request(format!(
        "A resource holds at most {MAX_TAGS_PER_RESOURCE} tags, and this request would \
         leave it with {would_hold}; remove a tag before adding another."
    ))
}
