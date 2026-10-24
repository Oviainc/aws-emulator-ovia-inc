// SPDX-License-Identifier: CC0-1.0

//! The modelled bounds of ONE `TagMap` entry, enforced where the request boundary cannot
//! reach them.
//!
//! `TagMap`'s key targets `TagKey` (`@length` min 1, max 128) and its value targets
//! `TagValue` (`@length` min 0, max 256)
//! (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`;
//! the four numbers are stated once, in [`crate::state`], and read from there here).
//!
//! The framework's boundary walk cannot refuse either bound: a map's KEY is captured as
//! the following value's path segment rather than checked, and a map's VALUE carries the
//! collection member's schema with its per-element check suppressed
//! (`vendor/smithy-schema-server/src/constraints.rs`,
//! `ValidatingSerializer::write_leaf`). So the handler that accepts the map is what must
//! refuse the entry, and every operation of this service that accepts a `TagMap` —
//! `CreateDomain` and `TagResource` — refuses it through [`violation`] here, stated once
//! so the two cannot enforce different halves of one bound.
//!
//! NOTHING about constraint checking or its wording is reimplemented: the two bounds are
//! re-declared in the framework's own form (a [`TraitMap`] carrying a [`LengthTrait`])
//! hung on a member schema named after the member the map is bound to, the check is
//! [`validate`] — the same walk the request boundary runs, so the length is counted in
//! characters and an over-long value is elided exactly as it would be there — and the
//! text is [`envelope`]'s. A refusal made here therefore reads exactly like one made at
//! the boundary.

use std::sync::LazyLock;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{path, validate, LengthTrait, Violation};
use ::aws_smithy_schema_server::envelope;

use crate::state::{MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, MIN_TAG_KEY_LEN, MIN_TAG_VALUE_LEN};

/// This service's Smithy namespace, for the member schemas below.
const NAMESPACE: &str = "com.amazonaws.connecthealth";

/// The member every operation of this service binds a `TagMap` to — `CreateDomainInput`
/// and `TagResourceInput` both spell it `tags` — which is the path a violation of one of
/// its entries is reported at.
const TAG_MAP_MEMBER: &str = "tags";

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

/// The refusal text for the first entry of `tags` outside the modelled `TagKey` /
/// `TagValue` lengths, or `None` when every entry is inside them — BOTH ends of both
/// bounds, so an empty key is refused and an empty value is not.
///
/// The caller renders the text through its own `ValidationException` channel (HTTP 400)
/// and writes nothing.
pub fn violation<'t>(
    tags: impl IntoIterator<Item = (&'t String, &'t String)>,
) -> Option<String> {
    tags.into_iter().find_map(|(key, value)| {
        // The key first: an entry whose key is outside its bound has no position to
        // report its value at.
        if let Some(violation) = checked(&M_TAG_KEY, key) {
            return envelope(&[violation]);
        }
        let mut violation = checked(&M_TAG_VALUE, value)?;
        // A map value's path is `<member>.<key>.member` — the spelling the walk itself
        // gives a map entry (`constraints::path::entry`), applied here because the key
        // is known only at request time.
        violation.path = format!("{TAG_MAP_MEMBER}.{}", path::entry(key));
        envelope(&[violation])
    })
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
}
