// SPDX-License-Identifier: CC0-1.0

//! The tag slice of this service's state.
//!
//! A web function's tags live in that function's own record
//! (`WebFunction::tags`), so `CreateWebFunction`, `TagResource`, `UntagResource`
//! and `ListTags` read and write ONE copy of the map and a function's delete
//! takes its tags with it. The two write paths share the rules below — the
//! reserved key namespace and the stored-map bound — rather than each carrying
//! its own copy.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aws_smithy_schema_server::constraints::path;

use super::ComAmazonawsLambdawebLambdawebState;

/// The tag key namespace a caller may not write to.
///
/// AWS reserves tag keys beginning with `aws:` for its own use: a caller can
/// neither create a tag whose key carries it nor change or remove one, on any
/// resource of any service. The model cannot express the rule — `TagKey`'s
/// pattern `^[a-zA-Z0-9 _\.:/=+\-@]+$` admits `:` (source:
/// `smithy_query(query="shape", shape="TagKey")`) — so the request boundary
/// passes such a key and the handlers apply the rule themselves.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What a tag key a CALLER names has to satisfy, as a refusal's clause states the
/// requirement: it is outside the namespace AWS reserves for the tags it applies
/// itself ([`RESERVED_TAG_KEY_PREFIX`]).
const TAG_KEY_REQUIREMENT: &str = "Member must not begin with the reserved prefix aws:";

/// The most tags one web function holds.
///
/// Cited, not chosen: `Tags` is `@length(min: 0, max: 200)` and
/// `ListTagsResponse$tags` targets it (source: `smithy_query(query="shape",
/// shape="Tags")`), so a function holding more tags than this could not be
/// reported by its own reader. A merge that would cross it is refused instead,
/// with the quota error `TagResource` models.
pub const MAX_TAGS_PER_FUNCTION: usize = 200;

/// Whether `key` names a tag AWS owns rather than one the caller may write.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// AWS's `ValidationException` message for ONE rule this service applies itself,
/// stating `requirement` of the value the caller sent at `member_path`.
///
/// The clause is the framework's own spelling for a violation that quotes its
/// value (`aws_smithy_schema_server::constraints::Violation::message`:
/// `Value '<value>' at '<path>' failed to satisfy constraint: <requirement>`), so
/// a rule the service applies reads exactly like a rule the request boundary
/// refuses; the envelope around it is `service_common`'s, which owns the count
/// word every service states a refusal with. The framework carries no
/// `ViolationKind` for a rule only this service can state — the enum is
/// `#[non_exhaustive]` and lives in that crate — which is why the requirement
/// after the clause is this service's own text.
fn stated_requirement(member_path: &str, submitted: &str, requirement: &str) -> String {
    let clause = format!(
        "Value '{submitted}' at '{member_path}' failed to satisfy constraint: {requirement}"
    );
    ::service_common::validation_message::envelope_over(std::slice::from_ref(&clause))
        .unwrap_or(clause)
}

/// Why a tag MAP a request carries may not be applied, or `None` when every key
/// of it is the caller's to write.
///
/// `member` is the model's name for the map member the keys arrived in, so the
/// clause names the member the caller sent. The keys are judged in KEY order
/// rather than in the order a hash map happens to yield them, so one request is
/// always refused for the same key, and the whole set is judged before anything
/// is written: a request carrying one reserved key among valid ones applies none
/// of them.
pub fn tag_map_violation<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let mut reserved: Vec<&str> = keys.into_iter().filter(|key| is_reserved_tag_key(key)).collect();
    reserved.sort_unstable();
    let key = reserved.first()?;
    // A map entry is named by the map member and the entry's own key, the way the
    // framework's validating walk names one.
    let entry = path::join(&[path::member(member), (*key).to_string()]);
    Some(stated_requirement(&entry, key, TAG_KEY_REQUIREMENT))
}

/// Why a tag KEY LIST a request carries may not be applied, or `None` when every
/// element of it is the caller's to remove.
///
/// The elements are judged in the order they arrived, because a list element is
/// named by its position: the clause names where the key sat.
pub fn tag_key_list_violation(member: &str, keys: &[String]) -> Option<String> {
    let (index, key) = keys
        .iter()
        .enumerate()
        .find(|(_, key)| is_reserved_tag_key(key))?;
    let element = path::join(&[path::member(member), path::element(index + 1)]);
    Some(stated_requirement(&element, key, TAG_KEY_REQUIREMENT))
}

/// The stored map as the modelled `Tags` shape: `HashMap`, since that is what the
/// generated input and output structures carry.
pub fn modeled_tag_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

impl ComAmazonawsLambdawebLambdawebState {
    /// The tags stored for `function_name`, or `None` when no such function is
    /// stored.
    ///
    /// A function that carries no tags answers an EMPTY map, which is a different
    /// answer from `None`: the function exists and holds nothing, and `Tags`
    /// admits the empty map (`@length(min: 0)`).
    pub fn tags(&self, function_name: &str) -> Option<&BTreeMap<String, String>> {
        self.functions.get(function_name).map(|f| &f.tags)
    }

    /// How many tags `function_name` would hold once `keys` were merged into its
    /// stored map, or `None` when no such function is stored.
    ///
    /// A key the function already carries is overwritten rather than added, so it
    /// is counted once — which is what keeps re-tagging an existing key clear of
    /// the bound in [`MAX_TAGS_PER_FUNCTION`].
    pub fn tag_count_after_merge<'a>(
        &self,
        function_name: &str,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<usize> {
        let stored = &self.functions.get(function_name)?.tags;
        // Only the keys the function does not already carry add to the count, and a
        // key named twice adds once.
        let added: BTreeSet<&str> = keys
            .into_iter()
            .filter(|key| !stored.contains_key(*key))
            .collect();
        Some(stored.len() + added.len())
    }

    /// Merge `tags` into `function_name`'s stored map, returning how many tags the
    /// function holds afterwards, or `None` when no such function is stored.
    ///
    /// A merge, not a replacement: a key the request names takes the value it
    /// carries ("If a tag key already exists, the existing value is overwritten
    /// with the new value.", source: `smithy_query(query="doc",
    /// shape="TagResource")`) and every key it does not name is left exactly as it
    /// was. The function's own `updatedAt` does not move: tags are addressed as
    /// their own sub-resource, like the resource-based policy, and the function's
    /// configuration is unchanged.
    pub fn merge_tags(
        &mut self,
        function_name: &str,
        tags: impl IntoIterator<Item = (String, String)>,
    ) -> Option<usize> {
        let function = self.functions.get_mut(function_name)?;
        function.tags.extend(tags);
        Some(function.tags.len())
    }

    /// Remove every key of `keys` from `function_name`'s stored map, returning how
    /// many of them were actually there, or `None` when no such function is
    /// stored.
    ///
    /// A key the function does not carry is passed over rather than refused, which
    /// is what makes the removal `@idempotent` as the model declares it: repeating
    /// it, or naming a key that was never set, leaves the same stored map.
    pub fn remove_tags<'a>(
        &mut self,
        function_name: &str,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<usize> {
        let function = self.functions.get_mut(function_name)?;
        let removed = keys
            .into_iter()
            .filter(|key| function.tags.remove(*key).is_some())
            .count();
        Some(removed)
    }
}
