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
//! The framework's boundary walk cannot refuse a tag's KEY: for a map the codec writes
//! the key against the map member's own schema before the value, so the walk captures it
//! as that value's path segment rather than checking it, and a list's scalar ELEMENT
//! carries the collection member's schema with its per-element check suppressed
//! (`vendor/smithy-schema-server/src/constraints.rs`,
//! `ValidatingSerializer::write_leaf`). A map's VALUE it does refuse, through the element
//! constraints the collection member's schema carries, reported at the entry's key
//! (`check_leaf`). So the handler that accepts the member is the only place a rule on a
//! KEY can be applied, and the value bound is stated beside it in the same one check —
//! both because an entry a handler is handed rather than deserialized has passed no walk,
//! and so that one place holds the whole of what a tag has to satisfy. Every operation of
//! this service that names a tag — `CreateDomain` and `TagResource` through
//! [`accepted_entries`], `UntagResource` through [`removed_key_violation`] — refuses it
//! here, so no two can enforce different halves of one bound.
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
//! That parity covers a request that breaks several rules at once. The boundary walk
//! COLLECTS every violation of one request and renders them into one envelope whose count
//! is the number of clauses it states, so a refusal made here collects every entry it
//! refuses too rather than stopping at whichever one it met first. The order is fixed by
//! the request's own content: a `TagMap` reaches a handler as an unordered map, whose
//! iteration order is neither the order the caller spelled nor stable between two maps
//! holding the same entries, so the entries are walked in order of their KEY — the one
//! order derivable from the entries themselves, which is why one request has exactly ONE
//! refusal text, whichever operation carried the map and however that map was built. A
//! `TagKeyList` arrives already ordered by the request, and its elements keep that order.
//! A refusal names at most [`MAX_NAMED_ENTRIES`] entries, so one request cannot size the
//! response it is answered with.
//!
//! The two classes are not mixed into one sentence: AWS's envelope states
//! modelled-constraint clauses, so a request that breaks a modelled length is answered by
//! that envelope, and the reserved-namespace sentence — the service's own words, naming
//! every key the request spells in that namespace — answers a request whose only fault is
//! the namespace.
//!
//! How many entries one resource may hold is the service's own rule too — `TagMap`
//! declares no `@length`, so the count is [`crate::state::MAX_TAGS_PER_RESOURCE`] — and
//! it is likewise compared in one place, [`merged_within_cap`]. What each operation
//! ANSWERS with stays its own: the count is refused through each operation's modelled
//! `ServiceQuotaExceededException`, which only that operation's error channel can
//! construct, so this module decides the condition and the handler words its own
//! sentence.
//!
//! The two halves are also stated in ONE ORDER, by [`accepted_entries`]: the per-entry
//! rule is decided from the request ALONE, which is where a modelled constraint is
//! decided — at the request boundary, before a handler runs at all — while the count is
//! decided from the set the addressed resource would end up holding, which is state. So
//! a handler runs [`accepted_entries`] before it reads any state, and compares the count
//! only afterwards; a map that breaks both rules at once is then refused for its entry,
//! with one status and one error code, whichever operation carries it.

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

/// How many entries one refusal names.
///
/// Every clause echoes the value it refused, so a refusal that named every entry of an
/// arbitrarily long request would turn one request into a response sized by it. The most
/// entries a caller could usefully be told about is the most a resource may hold
/// ([`MAX_TAGS_PER_RESOURCE`]) — a request naming more than that cannot be served whatever
/// its entries say — so collecting stops there, and the count a refusal states is the
/// number of entries it names.
const MAX_NAMED_ENTRIES: usize = MAX_TAGS_PER_RESOURCE;

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

/// The refusal text for the keys in the namespace the service reserves for the tags it
/// sets itself, or `None` when `keys` is empty.
///
/// The namespace is the service's own rule rather than a modelled constraint, so its
/// sentence is the service's own too: it names every key the caller asked for that is in
/// the namespace, the member that carried them, and the prefix no caller may use. ONE
/// sentence for every member a key can be spelled at, so naming a reserved key to SET it
/// and naming one to REMOVE it are refused the same way, and one request naming several
/// of them is answered once rather than told about an arbitrary one of them.
fn reserved_violation(member: &str, keys: &[String]) -> Option<String> {
    // One key is the sentence's subject, several are its subjects; the clause agrees with
    // the count the request carried rather than claiming a single offender.
    let (noun, verb) = match keys.len() {
        0 => return None,
        1 => ("key", "uses"),
        _ => ("keys", "use"),
    };
    let named: Vec<String> = keys.iter().map(|key| format!("'{key}'")).collect();
    Some(format!(
        "Tag keys cannot start with '{RESERVED_TAG_KEY_PREFIX}': the tag {noun} {} at \
         '{member}' {verb} the prefix reserved for tags this service sets",
        named.join(", ")
    ))
}

/// Everything the tag keys and values ONE request names do not satisfy — collected, not
/// stopped at, so a request that breaks several rules is answered by a refusal naming
/// every one of them, exactly as the boundary walk answers a request with several bad
/// members.
///
/// The two classes are kept apart because different authorities word them: a modelled
/// length is the framework's clause inside AWS's envelope, while the reserved namespace
/// is a rule no modelled constraint states and so is the service's own sentence.
#[derive(Debug, Default)]
struct Refused {
    /// Violations of a modelled `@length`, in the order the halves were walked in.
    lengths: Vec<Violation>,
    /// The keys that lie in the reserved namespace, in that same order.
    reserved: Vec<String>,
}

impl Refused {
    /// Whether this refusal already names as many entries as one refusal names.
    fn full(&self) -> bool {
        self.lengths.len() + self.reserved.len() >= MAX_NAMED_ENTRIES
    }

    /// Record whatever `key` does not satisfy, and report whether it satisfied
    /// everything — a caller holding the rest of the entry needs to know, since an entry
    /// whose key is refused has no position to report its value at.
    ///
    /// The ONE place the two per-key rules and their ORDER are stated, so every member a
    /// key can be spelled at is held to the same rules in the same sequence: the length
    /// first, because a key outside its bound has no further rule to answer to.
    ///
    /// `member_schema` carries `TagKey`'s bound under the name of the member that carried
    /// the key; `at` is the path a LENGTH violation is reported at when it is not that
    /// member's own name — a list element names its 1-based position, where a map's key
    /// names the member it is bound to.
    fn key(&mut self, member_schema: &'static Schema, key: &str, at: Option<String>) -> bool {
        if let Some(mut violation) = checked(member_schema, key) {
            if let Some(path) = at {
                violation.path = path;
            }
            self.lengths.push(violation);
            return true;
        }
        if reserved(key) {
            self.reserved.push(key.to_string());
            return true;
        }
        false
    }

    /// Record whatever the value of the entry keyed `key` does not satisfy. Only the KEY
    /// answers to the namespace rule — a VALUE may say anything its own length allows.
    fn value(&mut self, key: &str, value: &str) {
        if let Some(mut violation) = checked(&M_TAG_VALUE, value) {
            // A map value's path is `<member>.<key>.member` — the spelling the walk
            // itself gives a map entry (`constraints::path::entry`), applied here because
            // the key is known only at request time.
            violation.path = format!("{TAG_MAP_MEMBER}.{}", path::entry(key));
            self.lengths.push(violation);
        }
    }

    /// The refusal text for everything recorded, or `None` when nothing was: the
    /// framework's envelope over every modelled-constraint clause — so its count is the
    /// number of clauses it states — and otherwise the service's own sentence for the
    /// keys in the reserved namespace, reported at `member`.
    ///
    /// A request that breaks a modelled length is answered by the envelope ALONE, with
    /// the reserved keys it also named left unstated: the envelope's clauses are AWS's
    /// words for a modelled constraint and the namespace sentence is this service's, so
    /// appending one to the other would put a sentence no constraint states inside a count
    /// of constraint violations. The caller is told the whole of one class, and a request
    /// whose only fault is the namespace is told the whole of the other.
    fn text(&self, member: &str) -> Option<String> {
        envelope(&self.lengths).or_else(|| reserved_violation(member, &self.reserved))
    }
}

/// The refusal text for every one of `keys` this service does not accept, or `None` when
/// every key is acceptable — the check the one operation that names tag keys for REMOVAL
/// applies to its `tagKeys` list. At most [`MAX_NAMED_ENTRIES`] of them are named.
///
/// The per-key rules are exactly the ones [`violation`] applies to the keys of a
/// `TagMap`, because both go through [`Refused::key`]: a key outside the modelled
/// `TagKey` length, and a key in the reserved namespace, which names a tag no caller may
/// set OR remove. A length violation is reported at the element's own 1-based position in
/// the list — the spelling the boundary walk itself gives a list element
/// (`constraints::path::element`) — because a request may name up to fifty keys and the
/// caller has to know which one was refused. A list is ordered by the request, so the
/// clauses are reported in the order the caller spelled the keys in.
///
/// The caller renders the text through its own `ValidationException` channel (HTTP 400)
/// and writes nothing.
pub fn removed_key_violation<'k>(keys: impl IntoIterator<Item = &'k String>) -> Option<String> {
    let mut refused = Refused::default();
    for (index, key) in keys.into_iter().enumerate() {
        if refused.full() {
            break;
        }
        refused.key(
            &M_REMOVED_TAG_KEY,
            key,
            Some(format!(
                "{TAG_KEY_LIST_MEMBER}.{}",
                path::element(index + 1)
            )),
        );
    }
    refused.text(TAG_KEY_LIST_MEMBER)
}

/// The refusal text for every entry of `tags` this service does not accept — one outside
/// the modelled `TagKey` / `TagValue` lengths (BOTH ends of both bounds, so an empty key
/// is refused and an empty value is not), or one whose key lies in the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace — or `None` when every entry is acceptable.
///
/// Every refused entry is named, up to [`MAX_NAMED_ENTRIES`], and the entries are walked
/// in order of their KEY: `tags` reaches a handler as an unordered map, so the key is the
/// only ordering the entries themselves fix, and one request therefore has exactly one
/// refusal text — the same text whichever operation carried the map.
///
/// This is the CONTENT of the per-entry rule. A handler reaches it through
/// [`accepted_entries`], which is where the rule's place in a handler is stated: calling
/// this directly is what let two operations order the rule differently.
///
/// The caller renders the text through its own `ValidationException` channel (HTTP 400)
/// and writes nothing.
pub fn violation<'t>(
    tags: impl IntoIterator<Item = (&'t String, &'t String)>,
) -> Option<String> {
    let mut entries: Vec<(&String, &String)> = tags.into_iter().collect();
    entries.sort_by_key(|(key, _)| *key);
    let mut refused = Refused::default();
    for (key, value) in entries {
        if refused.full() {
            break;
        }
        // The key first, by the rule every key of this service answers to; an entry whose
        // key is refused contributes that refusal alone.
        if !refused.key(&M_TAG_KEY, key, None) {
            refused.value(key, value);
        }
    }
    refused.text(TAG_MAP_MEMBER)
}

/// The entries a request asks for, key-ordered and every one of them acceptable, or the
/// refusal text naming every entry this service does not accept ([`violation`]).
///
/// This states the per-entry rule's PLACE as well as its content, which is why both
/// operations that accept a `TagMap` enter the rule through here. The rule is decided
/// from the request ALONE — it is where a modelled constraint is decided, at the request
/// boundary, before a handler runs — so a handler calls this before it reads any state,
/// ahead of the count ([`merged_within_cap`]) and ahead of resolving the resource the
/// request addresses. A map that breaks the entry rule and a state-dependent rule at
/// once is therefore refused for its entry, with the one status and the one error code
/// the entry rule carries, whichever operation the map arrived on.
///
/// The entries are ordered by key before the walk, so a map of SEVERAL unacceptable
/// entries is refused with the same text however the request's map was ordered on the
/// wire — the operations cannot word one map's refusal differently.
///
/// The caller renders an `Err` through its own `ValidationException` channel (HTTP 400)
/// and hands an `Ok` on as the request's entries to merge; nothing is written here.
pub fn accepted_entries(
    requested: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>, String> {
    let requested: BTreeMap<String, String> = requested.into_iter().collect();
    match violation(requested.iter()) {
        Some(refusal) => Err(refusal),
        None => Ok(requested),
    }
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
