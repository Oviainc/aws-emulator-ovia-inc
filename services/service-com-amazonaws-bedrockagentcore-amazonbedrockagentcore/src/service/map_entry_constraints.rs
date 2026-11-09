// SPDX-License-Identifier: CC0-1.0

//! The modelled constraints the request boundary cannot reach: those a MAP's KEY shape
//! declares, and those declared on the members of the UNION a map's VALUES target.
//!
//! The framework's validating walk treats a map KEY as a path segment rather than as a
//! constrained leaf, and it does not descend into a map entry's VALUE, so a leaf inside one
//! reaches a handler unchecked. Storing such a value would accept a request the service
//! refuses, so every operation whose input declares one of these maps applies the MODEL's
//! own constraints here, before anything is persisted.
//!
//! The WORDS are not this module's: a refusal it finds is stated by the same builders the
//! request boundary states one with ([`crate::service::validation`], over
//! `service_common::validation_message`), so a constraint enforced in a handler is worded
//! exactly as the same constraint reported one level up and the two cannot drift.
//!
//! The member paths are the framework's own spelling of a position
//! (`aws_smithy_schema_server::constraints::path`): a map entry contributes `<key>.member`
//! and a list element its `<1-based index>.member`, so a clause worded here names the path
//! the walk would have named had it reached that leaf.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use service_common::validation::{quoted_echo, unanchored};
use service_common::validation_message::map_key_refusal;
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{MemoryRecordMetadataValue, MetadataValue};

/// The expression `MetadataKey`, `MetadataValue$stringValue`, `StringValue` and
/// `StringValueList`'s member all declare, as the model writes it (source: smithy_query
/// query="shape" shape="MetadataKey,MetadataValue,MemoryRecordMetadataValue,StringValue,
/// StringValueList").
const TEXT_PATTERN: &str = r"^[a-zA-Z0-9\s._:/=+@-]*$";

/// [`TEXT_PATTERN`] compiled once for the process: a model's expression is a fixed string,
/// so nothing a request carries can make the compile fail.
static TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(TEXT_PATTERN).expect("the model's expression compiles"));

/// `MetadataKey`, the key shape of both metadata maps: `@length` 1..128 and
/// [`TEXT_PATTERN`].
const METADATA_KEY: ConstrainedText = ConstrainedText {
    min_length: 1,
    max_length: 128,
    pattern: TEXT_PATTERN,
    regex: &TEXT,
};

/// `MetadataValue$stringValue`, the value an event's metadata entry carries: `@length`
/// 0..256 and [`TEXT_PATTERN`].
const EVENT_METADATA_TEXT: ConstrainedText = ConstrainedText {
    min_length: 0,
    max_length: 256,
    pattern: TEXT_PATTERN,
    regex: &TEXT,
};

/// `MemoryRecordMetadataValue$stringValue`, which targets `StringValue`: `@length` 1..256
/// and [`TEXT_PATTERN`].
const RECORD_METADATA_TEXT: ConstrainedText = ConstrainedText {
    min_length: 1,
    max_length: 256,
    pattern: TEXT_PATTERN,
    regex: &TEXT,
};

/// `StringValueList`'s member, `StringListMemberValue`: `@length` 1..64 and
/// [`TEXT_PATTERN`].
const RECORD_METADATA_LIST_ELEMENT: ConstrainedText = ConstrainedText {
    min_length: 1,
    max_length: 64,
    pattern: TEXT_PATTERN,
    regex: &TEXT,
};

/// `StringValueList`'s own `@length`, which bounds how many elements it holds.
const RECORD_METADATA_LIST_LENGTH: (i64, i64) = (1, 5);

/// How the framework renders the LIST it length-checked, which is what makes such a clause
/// drop its value half: a list has no scalar form to quote.
fn rendered_item_count(count: usize) -> String {
    format!("{count} items")
}

/// A string shape's declared `@length` bounds and `@pattern`.
#[derive(Debug, Clone, Copy)]
struct ConstrainedText {
    /// The `@length` minimum, in characters.
    min_length: i64,
    /// The `@length` maximum, in characters.
    max_length: i64,
    /// The `@pattern` as the MODEL writes it, anchors included: the CHECK runs the model's
    /// expression as written and only the RENDERING of a clause drops the anchors.
    pattern: &'static str,
    /// That same expression, compiled once.
    regex: &'static LazyLock<Regex>,
}

impl ConstrainedText {
    /// Every constraint `value` breaks, as violations at `path`, in the order the
    /// framework's own leaf walk reports them: the first failing `@length` bound, then the
    /// `@pattern`.
    fn violations(self, path: &str, value: &str) -> Vec<Violation> {
        let mut found = Vec::new();
        if let Some(kind) = self.length_violated_by(value) {
            found.push(violation(path, kind, value));
        }
        if !self.regex.is_match(value) {
            found.push(violation(
                path,
                ViolationKind::Pattern(self.pattern.to_string()),
                value,
            ));
        }
        found
    }

    /// The `@length` bound `value` broke, or `None`. `@length` on a string counts
    /// CHARACTERS, not bytes, which is the count the framework's own walk checks.
    fn length_violated_by(self, value: &str) -> Option<ViolationKind> {
        let length = i64::try_from(value.chars().count()).unwrap_or(i64::MAX);
        if length < self.min_length {
            return Some(ViolationKind::MinLength(self.min_length));
        }
        if length > self.max_length {
            return Some(ViolationKind::MaxLength(self.max_length));
        }
        None
    }

    /// Whether `value` satisfies everything this shape declares.
    fn admits(self, value: &str) -> bool {
        self.length_violated_by(value).is_none() && self.regex.is_match(value)
    }
}

/// One violation of `kind` at `path`, quoting `value` under the same ceiling every clause
/// of this service quotes a submitted value under.
fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
    Violation {
        path: path.to_string(),
        kind,
        value: Some(quoted_echo(value).into_owned()),
        sensitive: false,
    }
}

/// The whole message a `MetadataMap` at `member_path` is refused with, or `None` when every
/// entry satisfies what that map declares.
pub(crate) fn metadata_refusal(
    member_path: &str,
    entries: &HashMap<String, MetadataValue>,
) -> Option<String> {
    map_refusal(member_path, entries, |entry_path, value| match value {
        MetadataValue::stringValue(text) => {
            EVENT_METADATA_TEXT.violations(&format!("{entry_path}.stringValue"), text)
        }
    })
}

/// The whole message a `MemoryRecordMetadataMap` at `member_path` is refused with, or
/// `None` when every entry satisfies what that map declares.
///
/// `numberValue` and `dateTimeValue` declare no constraint of their own, so an entry
/// carrying one is admitted by its type alone.
pub(crate) fn record_metadata_refusal(
    member_path: &str,
    entries: &HashMap<String, MemoryRecordMetadataValue>,
) -> Option<String> {
    map_refusal(member_path, entries, |entry_path, value| match value {
        MemoryRecordMetadataValue::stringValue(text) => {
            RECORD_METADATA_TEXT.violations(&format!("{entry_path}.stringValue"), text)
        }
        MemoryRecordMetadataValue::stringListValue(values) => {
            string_list_violations(&format!("{entry_path}.stringListValue"), values)
        }
        MemoryRecordMetadataValue::numberValue(_)
        | MemoryRecordMetadataValue::dateTimeValue(_) => Vec::new(),
    })
}

/// Everything `StringValueList` declares, broken by `values` at `path`: its own `@length`
/// over the element count, then each element against the element shape.
///
/// Collecting stops at the ceiling one message may carry
/// ([`service_common::validation::MAX_VIOLATIONS`]), so a list the request boundary did not
/// reach cannot size the work this does or the sentence it produces.
fn string_list_violations(path: &str, values: &[String]) -> Vec<Violation> {
    let mut found = Vec::new();
    let (min, max) = RECORD_METADATA_LIST_LENGTH;
    let count = i64::try_from(values.len()).unwrap_or(i64::MAX);
    let bound = if count < min {
        Some(ViolationKind::MinLength(min))
    } else if count > max {
        Some(ViolationKind::MaxLength(max))
    } else {
        None
    };
    if let Some(kind) = bound {
        found.push(violation(path, kind, &rendered_item_count(values.len())));
    }
    for (at, value) in values.iter().enumerate() {
        if found.len() >= service_common::validation::MAX_VIOLATIONS {
            break;
        }
        found.extend(
            RECORD_METADATA_LIST_ELEMENT.violations(&format!("{path}.{}.member", at + 1), value),
        );
    }
    found
}

/// The whole message the entries of one map are refused with, or `None` when every entry
/// satisfies what the map declares.
///
/// A KEY outside what [`METADATA_KEY`] declares is answered on its own, at the MAP member:
/// the key is how an entry is addressed, so there is no path to word a clause about the
/// entry's value under. Everything else is one envelope over the violations the entries'
/// values broke.
///
/// The entries are walked in KEY order, not in the order a `HashMap` happens to hand them
/// over, so one request is always refused with one sentence.
fn map_refusal<V>(
    member_path: &str,
    entries: &HashMap<String, V>,
    violations_of: impl Fn(&str, &V) -> Vec<Violation>,
) -> Option<String> {
    let sorted: BTreeMap<&str, &V> = entries
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .collect();
    if sorted.keys().any(|key| !METADATA_KEY.admits(key)) {
        return Some(map_key_refusal(
            member_path,
            METADATA_KEY.min_length,
            METADATA_KEY.max_length,
            &unanchored(METADATA_KEY.pattern),
        ));
    }
    let violations: Vec<Violation> = sorted
        .iter()
        .flat_map(|(key, value)| violations_of(&format!("{member_path}.{key}.member"), value))
        .take(service_common::validation::MAX_VIOLATIONS)
        .collect();
    (!violations.is_empty())
        .then(|| crate::service::validation::stated_envelope(&violations))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value the model's expression admits is admitted, and every character outside
    /// its allowlist is refused — on the key shape and on a value shape alike.
    #[test]
    fn the_declared_expression_decides_which_text_is_admitted() {
        for (case, value, admitted) in [
            ("letters, digits and the punctuation the class lists", "web-chat_1.0/x=y+z:a@b", true),
            ("whitespace, which the class admits", "two words", true),
            ("the empty string, inside the event value's bounds", "", true),
            ("a hash, outside the class", "web#chat", false),
            ("a non-ascii letter, outside the class", "trag\u{fc}dien", false),
            ("a comma, outside the class", "a,b", false),
        ] {
            assert_eq!(
                EVENT_METADATA_TEXT.admits(value),
                admitted,
                "case {case:?}: expected admitted={admitted} for {value:?}"
            );
        }
        // The key shape declares the same expression and a minimum of one character.
        assert!(
            !METADATA_KEY.admits(""),
            "an empty key is shorter than the declared minimum"
        );
        assert!(
            METADATA_KEY.admits("channel"),
            "a key inside its bounds and its expression is admitted"
        );
    }

    /// A `@length` bound is counted in CHARACTERS: a value of 256 non-ascii characters is
    /// inside a 256-character maximum however many bytes it takes.
    #[test]
    fn a_length_bound_counts_characters() {
        let at_the_bound = "\u{e9}".repeat(256);
        assert_eq!(
            EVENT_METADATA_TEXT.length_violated_by(&at_the_bound),
            None,
            "256 characters is inside the declared maximum, got a violation"
        );
        let past_the_bound = "e".repeat(257);
        assert_eq!(
            EVENT_METADATA_TEXT.length_violated_by(&past_the_bound),
            Some(ViolationKind::MaxLength(256)),
            "257 characters is past the declared maximum"
        );
    }

    /// A list states its own element-count bound and every offending element, each at the
    /// path the framework spells for that position.
    #[test]
    fn a_string_list_states_its_count_bound_and_its_elements() {
        let too_many: Vec<String> = (0..6).map(|at| format!("v{at}")).collect();
        let found = string_list_violations("metadata.tags.member.stringListValue", &too_many);
        assert_eq!(
            found.len(),
            1,
            "expected only the count bound to be broken, got {found:?}"
        );
        assert_eq!(
            found[0].kind,
            ViolationKind::MaxLength(5),
            "expected the declared element-count maximum, got {:?}",
            found[0].kind
        );

        let offending = vec!["ok".to_string(), "no#".to_string()];
        let found = string_list_violations("metadata.tags.member.stringListValue", &offending);
        assert_eq!(
            found.len(),
            1,
            "expected one offending element, got {found:?}"
        );
        assert_eq!(
            found[0].path, "metadata.tags.member.stringListValue.2.member",
            "expected the second element's path, got {:?}",
            found[0].path
        );
    }

    /// An offending KEY is refused at the map member, whichever entry carried it, and a
    /// map whose entries are all inside their constraints is not refused at all.
    #[test]
    fn an_offending_key_is_refused_at_the_map_member() {
        let mut entries = HashMap::new();
        entries.insert(
            "chan#nel".to_string(),
            MetadataValue::stringValue("web".to_string()),
        );
        let refused = metadata_refusal("metadata", &entries).expect("the key is refused");
        assert!(
            refused.contains("at 'metadata' failed to satisfy constraint: Map keys must satisfy"),
            "expected a clause about the map's keys, got {refused:?}"
        );

        let mut entries = HashMap::new();
        entries.insert(
            "channel".to_string(),
            MetadataValue::stringValue("web-chat".to_string()),
        );
        assert_eq!(
            metadata_refusal("metadata", &entries),
            None,
            "a map inside its constraints is not refused"
        );
    }
}
