// SPDX-License-Identifier: CC0-1.0

//! What a tag a request NAMES has to satisfy — the modelled lengths of one entry's key
//! and value, the reserved key namespace the service keeps for itself, and how many
//! entries one resource may end up holding — enforced where the request boundary cannot
//! reach them.
//!
//! All three live here: [`violation`] is the per-ENTRY rule for an operation that writes
//! a `TagMap`, [`removed_key_violation`] the per-KEY rule for the one that names keys to
//! REMOVE, and [`merged_within_cap`] the per-RESOURCE count — each stated once, so no two
//! operations that name a tag can enforce different halves, or different readings, of one
//! rule.
//!
//! `TagMap`'s key targets `TagKey` (`@length` min 1, max 128) and its value targets
//! `TagValue` (`@length` min 0, max 256); `TagKeyList`, the list of keys to remove, is a
//! list of that same `TagKey`
//! (`smithy_query(query="shape",shape="TagMap,TagKeyList,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`;
//! the four numbers are stated once, in [`crate::state`], and read from there here).
//!
//! The framework's boundary walk cannot refuse either bound: a map's KEY is captured as
//! the following value's path segment rather than checked, and a map's VALUE — like a
//! list's scalar ELEMENT — carries the collection member's schema with its per-element
//! check suppressed
//! (`vendor/smithy-schema-server/src/constraints.rs`,
//! `ValidatingSerializer::write_leaf`). So the handler that accepts the member is what
//! must refuse it, and every operation of this service that names a tag — `CreateDomain`
//! and `TagResource` through [`violation`], `UntagResource` through
//! [`removed_key_violation`] — refuses it here, so no two can enforce different halves of
//! one bound.
//!
//! Beside the two modelled lengths there is one rule the model cannot state: the
//! [`crate::state::RESERVED_TAG_KEY_PREFIX`] namespace belongs to the tags the service
//! itself sets, so a key a caller spells with that prefix (whatever its case) is one no
//! caller may set OR remove, and a request naming one is refused rather than served.
//! `TagKey` carries no `@pattern`, so this is a service-side rule and the handler is
//! likewise the only place that can apply it — and it is applied in one place here, so
//! the operations that name a tag key cannot disagree about it either.
//!
//! NOTHING about constraint checking or its wording is reimplemented: the two bounds are
//! re-declared in the framework's own form (a [`TraitMap`] carrying a [`LengthTrait`])
//! hung on a member schema named after the member the tag is bound to, the check is
//! [`validate`] — the same walk the request boundary runs, so the length is counted in
//! characters and an over-long value is elided exactly as it would be there — and the
//! text is [`envelope`]'s. A refusal of a LENGTH made here therefore reads exactly like
//! one made at the boundary; the reserved-namespace refusal is the service's own sentence,
//! because no modelled constraint states that rule for it to borrow.
//!
//! How many entries one resource may hold is the service's own rule too — `TagMap`
//! declares no `@length`, so the count is [`crate::state::MAX_TAGS_PER_RESOURCE`] — and
//! it is likewise compared in one place, [`merged_within_cap`]. What each operation
//! ANSWERS with stays its own: the count is refused through each operation's modelled
//! `ServiceQuotaExceededException`, which only that operation's error channel can
//! construct, so this module decides the condition and the handler words its own
//! sentence.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{path, validate, LengthTrait, Violation};
use ::aws_smithy_schema_server::envelope;

use crate::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, MIN_TAG_KEY_LEN,
    MIN_TAG_VALUE_LEN, RESERVED_TAG_KEY_PREFIX,
};

/// This service's Smithy namespace, for the member schemas below.
const NAMESPACE: &str = "com.amazonaws.connecthealth";

/// The member every operation of this service binds a `TagMap` to — `CreateDomainInput`
/// and `TagResourceInput` both spell it `tags` — which is the path a violation of one of
/// its entries is reported at.
const TAG_MAP_MEMBER: &str = "tags";

/// The member the one operation that NAMES tag keys rather than tag entries binds its
/// `TagKeyList` to — `UntagResourceInput` spells it `tagKeys` — which is the path a
/// violation of one of its elements is reported at.
const TAG_KEY_LIST_MEMBER: &str = "tagKeys";

/// `TagKey`'s modelled `@length`, in the framework's own form.
static TAG_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(
        Some(MIN_TAG_KEY_LEN as i64),
        Some(MAX_TAG_KEY_LEN as i64),
    )));
    traits
});

/// `TagValue`'s modelled `@length`, in the framework's own form.
static TAG_VALUE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(
        Some(MIN_TAG_VALUE_LEN as i64),
        Some(MAX_TAG_VALUE_LEN as i64),
    )));
    traits
});

/// The schema a tag KEY is walked against: its name gives the violation path — so the
/// refusal names `tags`, exactly as a boundary refusal would — and its trait map carries
/// `TagKey`'s bound.
static M_TAG_KEY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.connecthealth#CheckedTagEntry$tags",
        NAMESPACE,
        "tags",
    ),
    ShapeType::String,
    TAG_MAP_MEMBER,
    0,
)
.with_traits(&TAG_KEY_TRAITS);

/// The schema a tag key named for REMOVAL is walked against: the same `TagKey` bound,
/// reported at the member that carried it (`tagKeys`) rather than at `tags`, since a
/// refusal has to name the member the caller spelled.
static M_REMOVED_TAG_KEY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.connecthealth#CheckedTagEntry$tagKeys",
        NAMESPACE,
        "tagKeys",
    ),
    ShapeType::String,
    TAG_KEY_LIST_MEMBER,
    2,
)
.with_traits(&TAG_KEY_TRAITS);

/// The schema a tag VALUE is walked against, carrying `TagValue`'s bound. The entry it
/// belongs to is added to the path afterwards (see [`violation`]), since the key is a
/// runtime value and a member schema is static.
static M_TAG_VALUE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.connecthealth#CheckedTagEntry$tagValue",
        NAMESPACE,
        "tagValue",
    ),
    ShapeType::String,
    TAG_MAP_MEMBER,
    1,
)
.with_traits(&TAG_VALUE_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`CheckedTagString`] WRITES, and a root struct schema
/// carries no member name, so it contributes no path segment. The shape id is one the
/// model does not declare, so it cannot collide with a modelled shape.
static CHECKED_TAG_ENTRY_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.connecthealth#CheckedTagEntry",
        NAMESPACE,
        "CheckedTagEntry",
    ),
    ShapeType::Structure,
    &[],
);

/// One half of one tag entry, written for the validating walk the way a codec writes a
/// string member.
struct CheckedTagString<'a> {
    member: &'static Schema,
    value: &'a str,
}

impl SerializableStruct for CheckedTagString<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(self.member, self.value)
    }
}

/// The violation the walk raises for `value` against `member`, or `None` when the value
/// is inside the member's bound. At most one: the schema carries one constraint and the
/// walk is driven over one leaf.
fn checked(member: &'static Schema, value: &str) -> Option<Violation> {
    validate(&CheckedTagString { member, value }, &CHECKED_TAG_ENTRY_SCHEMA)
        .into_iter()
        .next()
}

/// Whether `key` lies in the namespace the service reserves for the tags it sets itself
/// ([`RESERVED_TAG_KEY_PREFIX`]), compared without regard to ASCII case because the
/// namespace is reserved however the caller spells it. `str::get` yields `None` rather
/// than panicking when the prefix's length is not a character boundary of `key`, and a
/// key whose first characters are multi-byte is not in the namespace anyway.
fn reserved(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The refusal text for a key in the namespace the service reserves for the tags it
/// sets itself, or `None` when `key` is outside that namespace.
///
/// The namespace is the service's own rule rather than a modelled constraint, so its
/// sentence is the service's own too: it names the key the caller asked for, the member
/// that carried it, and the prefix no caller may use. ONE sentence for every member a
/// key can be spelled at, so naming a reserved key to SET it and naming one to REMOVE it
/// are refused the same way.
fn reserved_violation(member: &str, key: &str) -> Option<String> {
    reserved(key).then(|| {
        format!(
            "Tag keys cannot start with '{RESERVED_TAG_KEY_PREFIX}': the tag key '{key}' \
             at '{member}' uses the prefix reserved for tags this service sets"
        )
    })
}

/// The refusal text for a tag KEY this service does not accept — one outside the
/// modelled `TagKey` length (BOTH ends, so an empty key is refused) or one in the
/// reserved [`RESERVED_TAG_KEY_PREFIX`] namespace — or `None` when the key is
/// acceptable.
///
/// The ONE place the two per-key rules and their ORDER are stated, so every member a key
/// can be spelled at is held to the same rule in the same sequence: the length first,
/// because a key outside its bound has no further rule to answer to.
///
/// `member_schema` carries `TagKey`'s bound under the name of the member that carried the
/// key and `member` is that same name; `at` is the path a LENGTH refusal is reported at
/// when it is not that member's own name — a list element names its 1-based position,
/// where a map's key names the member it is bound to.
fn key_violation(
    member_schema: &'static Schema,
    member: &str,
    key: &str,
    at: Option<String>,
) -> Option<String> {
    if let Some(mut violation) = checked(member_schema, key) {
        if let Some(path) = at {
            violation.path = path;
        }
        return envelope(&[violation]);
    }
    reserved_violation(member, key)
}

/// The refusal text for the first of `keys` this service does not accept, or `None` when
/// every key is acceptable — the check the one operation that names tag keys for REMOVAL
/// applies to its `tagKeys` list.
///
/// The per-key rules are exactly the ones [`violation`] applies to the keys of a
/// `TagMap`, because both go through [`key_violation`]: a key outside the modelled
/// `TagKey` length, and a key in the reserved namespace, which names a tag no caller may
/// set OR remove. A length violation is reported at the element's own 1-based position in
/// the list — the spelling the boundary walk itself gives a list element
/// (`constraints::path::element`) — because a request may name up to fifty keys and the
/// caller has to know which one was refused.
///
/// The caller renders the text through its own `ValidationException` channel (HTTP 400)
/// and writes nothing.
pub fn removed_key_violation<'k>(keys: impl IntoIterator<Item = &'k String>) -> Option<String> {
    keys.into_iter().enumerate().find_map(|(index, key)| {
        key_violation(
            &M_REMOVED_TAG_KEY,
            TAG_KEY_LIST_MEMBER,
            key,
            Some(format!(
                "{TAG_KEY_LIST_MEMBER}.{}",
                path::element(index + 1)
            )),
        )
    })
}

/// The refusal text for the first entry of `tags` this service does not accept — one
/// outside the modelled `TagKey` / `TagValue` lengths (BOTH ends of both bounds, so an
/// empty key is refused and an empty value is not), or one whose key lies in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace — or `None` when every entry is acceptable.
///
/// The caller renders the text through its own `ValidationException` channel (HTTP 400)
/// and writes nothing.
pub fn violation<'t>(
    tags: impl IntoIterator<Item = (&'t String, &'t String)>,
) -> Option<String> {
    tags.into_iter().find_map(|(key, value)| {
        // The key first, by the rule every key of this service answers to: an entry
        // whose key is refused has no position to report its value at. Only the KEY is
        // subject to that rule — a VALUE may say anything its own length allows.
        if let Some(reported) = key_violation(&M_TAG_KEY, TAG_MAP_MEMBER, key, None) {
            return Some(reported);
        }
        let mut violation = checked(&M_TAG_VALUE, value)?;
        // A map value's path is `<member>.<key>.member` — the spelling the walk itself
        // gives a map entry (`constraints::path::entry`), applied here because the key
        // is known only at request time.
        violation.path = format!("{TAG_MAP_MEMBER}.{}", path::entry(key));
        envelope(&[violation])
    })
}

/// The tag set a request would leave a resource holding, or — when that set would be
/// larger than one resource may hold ([`MAX_TAGS_PER_RESOURCE`]) — how many entries it
/// would have carried.
///
/// `held` is the set the resource carries TODAY (empty for a resource that does not
/// exist yet, which is what a create addresses) and `requested` is the request's
/// entries, every key it names taking the value it gives and every key it does not name
/// keeping the value it had.
///
/// The cap is on the SET a resource ends up holding, not on one request's entry count,
/// so the merge happens first and its RESULT is what is bounded: re-stating keys the
/// resource already carries never exceeds the cap however many entries the request
/// carries, and the count reported back is the count the resource would have held. ONE
/// comparison, counted on one set, for every operation of this service that accepts a
/// `TagMap` — so a create and a later merge cannot bound different numbers.
///
/// Nothing is written here: a caller that is refused stores nothing, and one that is not
/// stores the set it gets back
/// ([`crate::state::ComAmazonawsConnecthealthConnecthealthState::set_tags`]).
pub fn merged_within_cap(
    held: BTreeMap<String, String>,
    requested: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>, usize> {
    let mut merged = held;
    merged.extend(requested);
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(merged.len());
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key outside its bound is refused at the member the map is bound to, a value
    /// outside its bound at its own entry, and each sentence is the framework's: the
    /// `1 validation error detected: …` envelope over the clause naming the value, the
    /// path and the bound that was violated.
    #[test]
    fn a_refused_entry_reads_like_a_boundary_refusal() {
        let key = String::new();
        let value = "v".to_string();
        let reported = violation([(&key, &value)]);
        assert_eq!(
            reported.as_deref(),
            Some(
                "1 validation error detected: Value '' at 'tags' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1"
            ),
            "an empty key must be refused at the tags member, got {reported:?}"
        );

        let key = "env".to_string();
        let value = "v".repeat(MAX_TAG_VALUE_LEN + 1);
        let reported = violation([(&key, &value)]);
        let expected = format!(
            "1 validation error detected: Value '{}... [{} bytes]' at 'tags.env.member' \
             failed to satisfy constraint: Member must have length less than or equal to {}",
            "v".repeat(256),
            value.len(),
            MAX_TAG_VALUE_LEN
        );
        assert_eq!(
            reported.as_deref(),
            Some(expected.as_str()),
            "an over-long value must be refused at its own entry, elided by the \
             framework's own ceiling, got {reported:?}"
        );
    }

    /// An entry inside both bounds is not refused, whichever form it takes — including
    /// the two the bounds admit at their minima: a one-character key and an empty value.
    #[test]
    fn an_entry_inside_both_bounds_is_not_refused() {
        for (case, key, value) in [
            ("an ordinary entry", "env".to_string(), "prod".to_string()),
            (
                // The namespace is reserved at the START of a key only, so a key that
                // merely contains the prefix is an ordinary key.
                "a key carrying the reserved prefix somewhere other than its start",
                format!("team{RESERVED_TAG_KEY_PREFIX}owner"),
                "prod".to_string(),
            ),
            (
                // And the rule is on the KEY: a value may say anything.
                "a value spelling the reserved prefix",
                "stack".to_string(),
                format!("{RESERVED_TAG_KEY_PREFIX}cloudformation"),
            ),
            ("a one-character key", "k".to_string(), "v".to_string()),
            ("an empty value", "retired".to_string(), String::new()),
            (
                "a key at the maximum length",
                "k".repeat(MAX_TAG_KEY_LEN),
                "v".to_string(),
            ),
            (
                "a value at the maximum length",
                "k".to_string(),
                "v".repeat(MAX_TAG_VALUE_LEN),
            ),
            (
                // `@length` counts characters, so a key of this many multi-byte
                // characters is inside the bound however many bytes it occupies.
                "a multi-byte key at the maximum CHARACTER length",
                "é".repeat(MAX_TAG_KEY_LEN),
                "v".to_string(),
            ),
        ] {
            let reported = violation([(&key, &value)]);
            assert!(
                reported.is_none(),
                "case {case:?}: must be accepted, got {reported:?}"
            );
        }
    }

    /// A key in the reserved namespace is refused whatever case it is spelled in, and the
    /// sentence names both the key the caller asked for and the prefix it may not use.
    #[test]
    fn a_key_in_the_reserved_namespace_is_refused() {
        let value = "anything".to_string();
        for spelling in ["aws:", "AWS:", "Aws:", "aWs:"] {
            let key = format!("{spelling}owner");
            let reported = violation([(&key, &value)]);
            let text = reported.unwrap_or_else(|| {
                panic!("case {key:?}: a reserved key must be refused, got nothing")
            });
            assert!(
                text.contains(&key) && text.contains(RESERVED_TAG_KEY_PREFIX),
                "case {key:?}: the refusal must name the key and the reserved prefix, \
                 got {text:?}"
            );
        }
    }

    /// A key named for REMOVAL answers to the same two rules as a key named for a write,
    /// and a length refusal names the element's own 1-based position in the list — so a
    /// caller naming many keys learns which one was refused.
    #[test]
    fn a_removed_key_is_held_to_the_same_rules_as_a_written_one() {
        let acceptable: Vec<String> = vec![
            "env".to_string(),
            "k".to_string(),
            "k".repeat(MAX_TAG_KEY_LEN),
            "é".repeat(MAX_TAG_KEY_LEN),
            "cost center".to_string(),
            // The namespace is reserved at the START of a key only.
            format!("team{RESERVED_TAG_KEY_PREFIX}owner"),
            // A key the resource does not carry is still a key the rules accept.
            "never-set".to_string(),
        ];
        let reported = removed_key_violation(acceptable.iter());
        assert!(
            reported.is_none(),
            "every acceptable key must be accepted for removal, got {reported:?}"
        );

        // A length refusal reads like a boundary refusal of the element that carried it.
        let over_long = "k".repeat(MAX_TAG_KEY_LEN + 1);
        let keys = ["env".to_string(), "team".to_string(), over_long.clone()];
        let reported = removed_key_violation(keys.iter());
        let expected = format!(
            "1 validation error detected: Value '{over_long}' at 'tagKeys.3.member' failed \
             to satisfy constraint: Member must have length less than or equal to {}",
            MAX_TAG_KEY_LEN
        );
        assert_eq!(
            reported.as_deref(),
            Some(expected.as_str()),
            "an over-long key must be refused at its own position, got {reported:?}"
        );

        // And the reserved namespace, at the member that carried the key.
        for spelling in ["aws:", "AWS:", "Aws:", "aWs:"] {
            let key = format!("{spelling}cloudformation");
            let keys = ["env".to_string(), key.clone()];
            let reported = removed_key_violation(keys.iter());
            let text = reported.unwrap_or_else(|| {
                panic!("case {key:?}: a reserved key must not be removable, got nothing")
            });
            assert!(
                text.contains(&key)
                    && text.contains(RESERVED_TAG_KEY_PREFIX)
                    && text.contains(TAG_KEY_LIST_MEMBER),
                "case {key:?}: the refusal must name the key, the reserved prefix and the \
                 member that carried it, got {text:?}"
            );
        }

        // An empty key is below the modelled minimum, at either end of the list.
        for (case, keys) in [
            ("alone", vec![String::new()]),
            ("after an acceptable key", vec!["env".to_string(), String::new()]),
        ] {
            let reported = removed_key_violation(keys.iter());
            assert!(
                reported.is_some_and(|text| text.starts_with("1 validation error detected: ")),
                "case {case:?}: an empty key must be refused"
            );
        }
    }
}
