// SPDX-License-Identifier: CC0-1.0

//! The tag contract the two tag-WRITING operations refuse a request against, and
//! the error channel they refuse it in.
//!
//! A refusal has to be stated in an error code the CALLED operation declares,
//! because that is the only vocabulary a generated SDK gives its caller to catch.
//! `TagResource` and `UntagResource` are the two operations of this service whose
//! model declares `BadRequestException` (400) and `NotFoundException` (404) and
//! nothing else — `smithy_query(query="operation",operation="TagResource,UntagResource",
//! model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")`
//! — while nearly every other operation here declares `InvalidParameterException`,
//! which is what `crate::service::EmulatedService::override_refusal` states a
//! boundary refusal as. That seam is per SERVICE and is reached after the framework
//! has already refused, with only the violations in hand and no operation named
//! (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), so it cannot tell the two
//! apart. This module is where they are told apart: the one seam a service has
//! BEFORE dispatch (`Service::override_operation`) claims a tag request the contract
//! refuses and answers it as `BadRequestException`, exactly as
//! `services/service-com-amazonaws-xray-awsxray` claims a request whose body its
//! own request layer refuses.
//!
//! Two kinds of refusal arrive here, and both are the model's own rules:
//!
//! * the ones the framework's validating walk finds — the entry count of `tags` and
//!   the element count of `tagKeys`, each bounded 1..50, and an absent `@required`
//!   member. The walk is run HERE, through the same
//!   `aws_smithy_schema_server::constraints::validate` and the same generated input
//!   schema the dispatch sequence runs, so this layer cannot disagree with the walk
//!   that would otherwise follow it;
//! * the ones that walk cannot word correctly, and the one it does not reach at all:
//!   a map KEY is not captured by it, so the `TagKey` bound (1..128) goes unchecked,
//!   and a map VALUE it does check it words as one more constrained leaf
//!   (`vendor/smithy-schema-server/src/constraints.rs`). Both halves are checked
//!   here, against the same bounds the model declares — `TagKey` (1..128) and
//!   `TagValue` (0..256) — so the two legs read alike.
//!
//! Three wordings answer those refusals, and which one a refusal earns follows
//! WHAT was broken:
//!
//! * the entry-count ceiling of the tag map is stated in this service's own tagging
//!   sentence ([`crate::state::tag_ceiling_message`]): the limit is its only variable
//!   part, and it echoes nothing the request carried;
//! * a per-entry bound of the tag map being ADDED — a key against `TagKey`, a value
//!   against `TagValue` — is stated in the tagging front end's map-constraint
//!   wording ([`added_tags_refusal`]), which the recorded service answers: the
//!   validation envelope carried as a JSON DOCUMENT in the message, the offending
//!   ENTRY rendered `{key=value}`, the front end's own member name for the tags being
//!   added, and the broken bound stated as a map constraint carrying BOTH the
//!   modelled maximum and minimum;
//! * every other refusal keeps the constraint report the framework already words,
//!   which names the member and the bound it broke.

use std::collections::HashMap;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{envelope, path, validate, Violation, ViolationKind};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, OperationContext, Refused,
    ServerHttpProtocol, Service,
};

use crate::ops::types::{
    TagResourceInput, UntagResourceInput, TAG_RESOURCE_INPUT_SCHEMA, UNTAG_RESOURCE_INPUT_SCHEMA,
};
use crate::state::tag_ceiling_message;

/// The error code these two operations refuse a bad request with: the only 400 their
/// model declares.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The member `TagResource` carries the tags to add in, and `UntagResource` the keys
/// to remove in — the names the model gives them, which are also the names a
/// constraint clause reports a member under.
const TAGS_MEMBER: &str = "tags";
/// See [`TAGS_MEMBER`].
const TAG_KEYS_MEMBER: &str = "tagKeys";

/// The character bounds the model declares on one tag key: `TagKey` is
/// `@length {min: 1, max: 128}` and is the key of `TagMap` and the member of
/// `TagKeyList` (`smithy_query(query="shape",shape="TagMap,TagKey,TagKeyList",…)`).
const MIN_TAG_KEY_LENGTH: i64 = 1;
/// See [`MIN_TAG_KEY_LENGTH`].
const MAX_TAG_KEY_LENGTH: i64 = 128;

/// The character bounds the model declares on one tag value: `TagValue` is
/// `@length {min: 0, max: 256}` (`smithy_query(query="shape",shape="TagValue",…)`).
/// The low bound is zero — an empty value is a tag with no descriptor, which the
/// model allows — so only the high bound can be broken.
const MIN_TAG_VALUE_LENGTH: i64 = 0;
/// See [`MIN_TAG_VALUE_LENGTH`].
const MAX_TAG_VALUE_LENGTH: i64 = 256;

/// The member name the tagging front end reports a `TagResource` request's tag map
/// under.
///
/// It is the front end's own name for the tags being ADDED, not the input member
/// `tags` the model declares — a per-entry refusal of this map names this member, and
/// a client that reads the member out of the refusal reads this name. RECORDED
/// BEHAVIOUR of the tagging front end; the model names no path for it.
const ADDED_TAGS_MEMBER: &str = "addedTags";

/// How much of a caller-supplied key or value one clause echoes: the cap every
/// refusal this service words shares ([`crate::state::MAX_ECHOED_VALUE`]).
const MAX_ECHOED_VALUE: usize = crate::state::MAX_ECHOED_VALUE;

/// `value` elided at `limit` characters, on a character boundary, with the elision
/// marked — the one echo form this service words every refusal in
/// ([`crate::state::elided_value`]).
fn elided(value: &str, limit: usize) -> String {
    crate::state::elided_value(value, limit)
}

/// `value` as a clause may echo it: elided at [`MAX_ECHOED_VALUE`] characters.
fn echoed(value: &str) -> String {
    elided(value, MAX_ECHOED_VALUE)
}

/// One refused tag request: what it broke, and how this service states it.
///
/// The violations travel with the message because the framework's renderer takes
/// both — the structured failure decides the STATUS (a broken bound is a 400) and
/// the envelope, and the chosen code and message decide what the caller reads.
#[derive(Debug)]
pub struct TagRefusal {
    violations: Vec<Violation>,
    message: String,
}

/// Why the request could not be read into the modelled input shape, when it could
/// not.
enum Read<T> {
    /// The input the request carries.
    Value(T),
    /// The read failed in a way the model describes structurally (an absent
    /// `@required` member), so it is a refusal this layer states.
    Refused(TagRefusal),
    /// The read failed in a way the model does not describe — a body the codec
    /// cannot process at all. Not this layer's refusal: the framework answers it in
    /// its own envelope.
    Unreadable,
}

/// Read the request into `T` through the protocol and schema the dispatch sequence
/// uses, so this layer reads exactly the value the handler would be given.
fn read<T: DeserializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    schema: &'static Schema,
    req: &RtRequest,
) -> Read<T> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, schema) else {
        return Read::Unreadable;
    };
    match T::deserialize(&mut *deserializer) {
        Ok(value) => Read::Value(value),
        Err(error) => match smithy_server_runtime::refusal_of(&ProtocolError::Serde(error)) {
            // An absent `@required` member is a rule the model declares, and the
            // framework already words it as a constraint clause.
            Some(smithy_server_runtime::Refusal::Invalid(violations)) => {
                match reported_refusal(violations) {
                    Some(refusal) => Read::Refused(refusal),
                    None => Read::Unreadable,
                }
            }
            _ => Read::Unreadable,
        },
    }
}

/// How this service states `violations` the framework's own walk over the whole
/// input found, or `None` when it found none.
///
/// This is the one path on which the tag-count ceiling can be reached, because the
/// entry count of a map is checked by that walk and nowhere else.
fn declared_refusal(violations: Vec<Violation>) -> Option<TagRefusal> {
    let reported = envelope(&violations)?;
    let message = match tag_count_ceiling(&violations) {
        Some(limit) => tag_ceiling_message(limit),
        None => reported,
    };
    Some(TagRefusal {
        violations,
        message,
    })
}

/// How this service states `violations` as the constraint report the framework words
/// for every other refusal, or `None` when there are none.
fn reported_refusal(violations: Vec<Violation>) -> Option<TagRefusal> {
    let message = envelope(&violations)?;
    Some(TagRefusal {
        violations,
        message,
    })
}

/// The modelled maximum the tag MAP's entry count broke, when that is what the walk
/// refused.
///
/// The limit is read off the violation rather than written down, so the sentence
/// states the bound the model declares and the two cannot drift. A bound broken
/// anywhere else — a tag key's length, the element count of `tagKeys` — is not this
/// refusal and keeps the generic constraint report.
fn tag_count_ceiling(violations: &[Violation]) -> Option<usize> {
    violations.iter().find_map(|violation| match violation.kind {
        ViolationKind::MaxLength(max) if violation.path == TAGS_MEMBER => usize::try_from(max).ok(),
        _ => None,
    })
}

/// Which bound of the modelled `@length` a caller-supplied string broke, or `None`
/// when it broke neither.
///
/// `@length` on a string counts CHARACTERS, not bytes ("Maximum key length – 128
/// Unicode characters in UTF-8", `TagMap`), which is also how the framework's own
/// walk counts them. The ONE place this comparison lives, so the per-element and the
/// per-entry checks cannot judge the same string differently.
fn broken_bound(shown: &str, min: i64, max: i64) -> Option<ViolationKind> {
    let length = i64::try_from(shown.chars().count()).unwrap_or(i64::MAX);
    if length < min {
        Some(ViolationKind::MinLength(min))
    } else if length > max {
        Some(ViolationKind::MaxLength(max))
    } else {
        None
    }
}

/// The clause a caller-supplied string earns when its character count falls outside
/// the modelled `@length` of the shape it is bound to, or `None` when it is inside.
fn length_clause(member_path: String, shown: &str, min: i64, max: i64) -> Option<Violation> {
    let kind = broken_bound(shown, min, max)?;
    Some(Violation {
        path: member_path,
        kind,
        value: Some(echoed(shown)),
        sensitive: false,
    })
}

/// Which half of a map entry a clause is about.
///
/// The two halves are bound to DIFFERENT shapes — a key to `TagKey`, a value to
/// `TagValue` — so each carries its own modelled bounds, and the map-constraint
/// wording names the half it is about. Both travel with the half rather than with
/// each call site, so the bounds a clause states cannot drift from the shape it
/// checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryHalf {
    /// The key an entry is filed under, bound to `TagKey`.
    Key,
    /// The value an entry holds, bound to `TagValue`.
    Value,
}

impl EntryHalf {
    /// The modelled `@length` of the shape this half is bound to, as (min, max).
    const fn bounds(self) -> (i64, i64) {
        match self {
            Self::Key => (MIN_TAG_KEY_LENGTH, MAX_TAG_KEY_LENGTH),
            Self::Value => (MIN_TAG_VALUE_LENGTH, MAX_TAG_VALUE_LENGTH),
        }
    }

    /// How much of this half a rendered entry carries: TWICE the maximum its shape
    /// allows.
    ///
    /// A clause shows the caller the entry that was refused, so a half that overran
    /// its bound has to be readable as sent — a limit AT the bound would elide every
    /// refused value there is. Twice it is model-derived rather than chosen, and it
    /// bounds the document by the MODEL instead of by how much the caller sent, which
    /// is what keeps one request from being amplified into an arbitrarily long
    /// response.
    fn echo_limit(self) -> usize {
        let (_, max) = self.bounds();
        usize::try_from(max.saturating_mul(2)).unwrap_or(usize::MAX)
    }

    /// How a map-constraint clause names this half.
    const fn subject(self) -> &'static str {
        match self {
            Self::Key => "Map keys",
            Self::Value => "Map value",
        }
    }
}

/// One clause the tagging front end states about one entry of the tag map.
///
/// The structured [`Violation`] and the sentence travel together because they answer
/// different questions: the violation is the model-level fact that decides the STATUS
/// and the envelope, and the clause is what the caller reads.
#[derive(Debug)]
struct EntryClause {
    violation: Violation,
    clause: String,
}

/// One half of an entry as the refusal DOCUMENT carries it: elided at that half's
/// own [`EntryHalf::echo_limit`], with the two characters a JSON string cannot hold
/// raw escaped so the document a caller parses stays well formed whatever the request
/// put in the map, and with the quote this document escapes everywhere escaped too.
fn in_document(value: &str, half: EntryHalf) -> String {
    elided(value, half.echo_limit())
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\'', "\\'")
}

/// The clause one entry earns when `half` falls outside the modelled `@length` of the
/// shape that half is bound to, or `None` when it is inside.
///
/// The clause states the whole offending ENTRY and BOTH modelled bounds, whichever of
/// the two was broken — that pair is fixed text in this wording, and only the entry and
/// the bounds vary.
fn entry_clause(key: &str, value: &str, half: EntryHalf) -> Option<EntryClause> {
    let (min, max) = half.bounds();
    let shown = match half {
        EntryHalf::Key => key,
        EntryHalf::Value => value,
    };
    let kind = broken_bound(shown, min, max)?;
    let entry = format!(
        "{{{}={}}}",
        in_document(key, EntryHalf::Key),
        in_document(value, EntryHalf::Value)
    );
    let subject = half.subject();
    let clause = format!(
        "Value \\'{entry}\\' at \\'{ADDED_TAGS_MEMBER}\\' failed to satisfy constraint: \
         {subject} must satisfy constraint: \
         [Member must have length less than or equal to {max}, \
         Member must have length greater than or equal to {min}]"
    );
    Some(EntryClause {
        violation: Violation {
            path: path::member(TAGS_MEMBER),
            kind,
            value: Some(echoed(shown)),
            sensitive: false,
        },
        clause,
    })
}

/// Every per-entry bound a tag map breaks: each key against `TagKey` and each value
/// against `TagValue`.
///
/// The keys are walked in sorted order so one request always earns the same clauses
/// in the same order, whatever order the map happens to iterate in.
fn tag_map_clauses(tags: &HashMap<String, String>) -> Vec<EntryClause> {
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    let mut clauses = Vec::new();
    for key in keys {
        let Some(value) = tags.get(key) else { continue };
        clauses.extend(entry_clause(key, value, EntryHalf::Key));
        clauses.extend(entry_clause(key, value, EntryHalf::Value));
    }
    clauses
}

/// The validation envelope as the tagging front end CARRIES it: a JSON document in
/// the message, with every quote inside it escaped.
///
/// The count prefix is the envelope's own
/// (`aws_smithy_schema_server::constraints::envelope`), restated here because that
/// renderer words each clause in the scalar-member form and this wording states a map
/// constraint instead — the prefix is all the two share.
fn validation_document(clauses: &[String]) -> Option<String> {
    let detected = match clauses.len() {
        0 => return None,
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    };
    Some(format!("{{\"message\": \"{detected}\"}}"))
}

/// How this service refuses a tag map whose entries break the per-entry bounds the
/// model declares, or `None` when none of them do.
fn added_tags_refusal(tags: &HashMap<String, String>) -> Option<TagRefusal> {
    let clauses = tag_map_clauses(tags);
    let sentences: Vec<String> = clauses
        .iter()
        .map(|clause| clause.clause.clone())
        .collect();
    let message = validation_document(&sentences)?;
    Some(TagRefusal {
        violations: clauses
            .into_iter()
            .map(|clause| clause.violation)
            .collect(),
        message,
    })
}

/// Every per-element bound a list of tag keys breaks: each element against `TagKey`.
///
/// An element reports under its 1-based position, which is the segment the
/// framework's walk names a list element with.
fn tag_key_list_clauses(tag_keys: &[String]) -> Vec<Violation> {
    tag_keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| {
            length_clause(
                path::join(&[path::member(TAG_KEYS_MEMBER), path::element(index + 1)]),
                key,
                MIN_TAG_KEY_LENGTH,
                MAX_TAG_KEY_LENGTH,
            )
        })
        .collect()
}

/// The bounds the model declares on the whole input shape, checked by the
/// framework's own validating walk.
fn declared(input: &dyn SerializableStruct, schema: &'static Schema) -> Option<TagRefusal> {
    declared_refusal(validate(input, schema))
}

/// How this service refuses the request `key` names, or `None` when the tag contract
/// has nothing to refuse about it.
///
/// `key` is the routing key the protocol resolved, which on this service's restJson1
/// protocol is the operation's own shape name
/// (`smithy_query(query="digest",…)` reports `restJson1`), so the match below is the
/// one place this module names the two operations it answers for.
pub fn refusal_for(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<TagRefusal> {
    match key {
        "TagResource" => {
            let input: TagResourceInput =
                match read(protocol, TAG_RESOURCE_INPUT_SCHEMA, req) {
                    Read::Value(input) => input,
                    Read::Refused(refusal) => return Some(refusal),
                    Read::Unreadable => return None,
                };
            // The walk over the whole input runs FIRST, as it does for the other
            // operation: it is the only thing that can reach the tag map's
            // entry-count ceiling, which outranks a per-entry bound.
            let walked = validate(&input, TAG_RESOURCE_INPUT_SCHEMA);
            if tag_count_ceiling(&walked).is_some() {
                return declared_refusal(walked);
            }
            // A per-entry bound of the map being added is worded by the tagging
            // front end's map-constraint renderer, not by the walk's scalar-member
            // clause: the walk sees a map value as one more constrained leaf and
            // does not see a map key at all. Checking both halves here is what makes
            // the two legs read alike, and it is the same bound either way.
            added_tags_refusal(&input.tags).or_else(|| reported_refusal(walked))
        }
        "UntagResource" => {
            let input: UntagResourceInput =
                match read(protocol, UNTAG_RESOURCE_INPUT_SCHEMA, req) {
                    Read::Value(input) => input,
                    Read::Refused(refusal) => return Some(refusal),
                    Read::Unreadable => return None,
                };
            declared(&input, UNTAG_RESOURCE_INPUT_SCHEMA)
                .or_else(|| reported_refusal(tag_key_list_clauses(&input.tag_keys)))
        }
        _ => None,
    }
}

/// The operation this service serves for a tag request its own tag contract refuses.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a BORROWED
/// operation. It holds no state: the refusal is re-derived from the same request the
/// router passes back into [`ErasedOperation::run`], so one value serves every such
/// request.
#[derive(Debug, Default)]
pub struct RefusedTagRequest;

impl RefusedTagRequest {
    /// The one refusal value a service holds.
    pub const fn new() -> Self {
        Self
    }
}

impl ErasedOperation for RefusedTagRequest {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service's model-derived half is read inside this block and not
            // held past it: `&dyn ErasedServiceDefinition` is not `Sync`, and the
            // future returned here is `Send`. Only the modelled operation — which
            // is `Send + Sync` — crosses the await below.
            let answer = {
                let definition = svc.definition();
                let protocol = definition.protocol();
                let key = protocol.routing_key(req).ok();
                // The body is read with the codec dispatch reads one with (see
                // [`crate::service::body_codec`]).
                let body = super::body_codec(svc, definition);
                // The same decision `override_operation` made, re-derived from the
                // same request: this operation is reached only when it answered
                // `Some`, so the refusal is found again. If it somehow is not, the
                // modelled operation serves the request as it would have anyway.
                match key.and_then(|key| refusal_for(key, body, req)) {
                    Some(refusal) => Ok(definition.refusal_response(
                        ProtocolError::Invalid(refusal.violations),
                        Some(Refused {
                            code: BAD_REQUEST_EXCEPTION.to_string(),
                            message: refusal.message,
                        }),
                        req,
                    )),
                    None => match key.and_then(|key| definition.routing_table().get(key)) {
                        Some(operation) => Err(operation),
                        None => Ok(definition.routing_error_response(req)),
                    },
                }
            };
            match answer {
                Ok(response) => response,
                Err(operation) => operation.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so this
        // refusal cannot weaken the schemes the model declares for it.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map of `count` distinct keys, each with a value.
    fn map(count: usize) -> HashMap<String, String> {
        (0..count)
            .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
            .collect()
    }

    /// The modelled per-entry bounds, over a row per edge: each published bound of
    /// `TagKey` (1..128) and `TagValue` (0..256), and one character past each.
    #[test]
    fn the_modelled_per_entry_bounds_decide_a_tag_map() {
        // (case, key length, value length, how many clauses the entry earns)
        let rows: [(&str, usize, usize, usize); 8] = [
            ("both bounds' low end", 1, 0, 0),
            ("both bounds' high end", 128, 256, 0),
            ("a key one past its high bound", 129, 0, 1),
            ("a key far past its high bound", 1_000, 0, 1),
            ("an empty key, one under its low bound", 0, 0, 1),
            ("a value one past its high bound", 1, 257, 1),
            ("a value far past its high bound", 1, 10_000, 1),
            ("a key and a value both past theirs", 129, 257, 2),
        ];

        for (case, key_length, value_length, expected) in rows {
            let mut tags = HashMap::new();
            tags.insert("x".repeat(key_length), "v".repeat(value_length));
            let clauses = tag_map_clauses(&tags);
            assert_eq!(
                clauses.len(),
                expected,
                "case {case:?}: expected {expected} clauses, got {clauses:?}"
            );
            for clause in &clauses {
                assert!(
                    clause
                        .violation
                        .value
                        .as_deref()
                        .is_some_and(|shown| shown.chars().count() <= MAX_ECHOED_VALUE + 1),
                    "case {case:?}: a clause must not echo an unbounded value, got {clause:?}"
                );
            }
        }
    }

    /// A caller-supplied key or value cannot break the DOCUMENT the refusal carries:
    /// the two characters a JSON string cannot hold raw are escaped, so what a client
    /// parses out of the message stays a document.
    #[test]
    fn a_quote_in_a_tag_cannot_break_the_document() {
        // (case, the characters the key ends with, how the entry must render them)
        let rows: [(&str, &str, &str); 3] = [
            ("a double quote in the key", "\"", "\\\""),
            ("a backslash in the key", "\\", "\\\\"),
            ("both in the key", "\"\\", "\\\"\\\\"),
        ];
        for (case, injected, rendered) in rows {
            // The key is over its bound by the characters injected into it, so the
            // entry is rendered.
            let stem = "k".repeat(MAX_TAG_KEY_LENGTH as usize);
            let mut tags = HashMap::new();
            tags.insert(format!("{stem}{injected}"), "v".to_string());
            let refusal = added_tags_refusal(&tags).expect("a refusal");
            let entry = format!("{{{stem}{rendered}=v}}");
            assert!(
                refusal.message.contains(&entry),
                "case {case:?}: expected the escaped entry {entry:?}, got {:?}",
                refusal.message
            );
        }
    }

    /// The entry-count ceiling is the one refusal stated in this service's own
    /// tagging sentence; a bound broken anywhere else keeps the constraint report.
    #[test]
    fn the_entry_count_ceiling_is_the_only_refusal_with_its_own_sentence() {
        // What the walk over the whole input can report at the tag map: its entry
        // count, the member being absent, and an entry's value under the entry's own
        // path segment.
        let over_limit = Violation {
            path: TAGS_MEMBER.to_string(),
            kind: ViolationKind::MaxLength(50),
            value: Some("51 entries".to_string()),
            sensitive: false,
        };
        let absent = Violation {
            path: TAGS_MEMBER.to_string(),
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        };
        let long_value = Violation {
            path: path::join(&[path::member(TAGS_MEMBER), "env".to_string()]),
            kind: ViolationKind::MaxLength(MAX_TAG_VALUE_LENGTH),
            value: Some("v".repeat(257)),
            sensitive: false,
        };

        let ceiling = declared_refusal(vec![over_limit.clone()]).expect("a refusal");
        assert_eq!(
            ceiling.message,
            tag_ceiling_message(50),
            "the entry-count ceiling states the tagging sentence, got {:?}",
            ceiling.message
        );

        for (case, violation) in [("an absent member", absent), ("a long value", long_value)] {
            let refusal = declared_refusal(vec![violation]).expect("a refusal");
            assert!(
                refusal.message.starts_with("1 validation error detected: "),
                "case {case:?}: expected the constraint report, got {:?}",
                refusal.message
            );
        }

        // The per-entry path reports EVERY clause it raises, including one shaped like
        // the entry count, which only the walk over the whole input can raise.
        let long_key = Violation {
            path: TAGS_MEMBER.to_string(),
            kind: ViolationKind::MaxLength(MAX_TAG_KEY_LENGTH),
            value: Some("x".repeat(129)),
            sensitive: false,
        };
        for (case, violation) in [
            ("a long key", long_key),
            ("a clause shaped like the ceiling", over_limit),
        ] {
            let refusal = reported_refusal(vec![violation]).expect("a refusal");
            assert!(
                refusal.message.starts_with("1 validation error detected: "),
                "case {case:?}: expected the constraint report, got {:?}",
                refusal.message
            );
        }

        assert!(
            declared_refusal(Vec::new()).is_none(),
            "no violations is no refusal"
        );
        assert!(
            reported_refusal(Vec::new()).is_none(),
            "no violations is no refusal"
        );
    }

    /// Only the entry count can carry the tagging sentence, and the limit it states
    /// is the one the violation carries — never a number written down here.
    #[test]
    fn the_stated_limit_follows_the_violation() {
        for limit in [1_i64, 10, 50, 100] {
            let found = tag_count_ceiling(&[Violation {
                path: TAGS_MEMBER.to_string(),
                kind: ViolationKind::MaxLength(limit),
                value: Some(format!("{} entries", limit + 1)),
                sensitive: false,
            }]);
            assert_eq!(
                found,
                Some(limit as usize),
                "case {limit}: expected the violation's own limit, got {found:?}"
            );
        }

        // A bound broken on another member is not the tag-count ceiling.
        let elsewhere = tag_count_ceiling(&[Violation {
            path: path::join(&[path::member(TAG_KEYS_MEMBER), path::element(1)]),
            kind: ViolationKind::MaxLength(MAX_TAG_KEY_LENGTH),
            value: Some("x".repeat(129)),
            sensitive: false,
        }]);
        assert_eq!(
            elsewhere, None,
            "a clause on another member is not the entry-count ceiling, got {elsewhere:?}"
        );
    }

    /// A map inside the modelled entry count earns no per-entry clause at all, at
    /// every size the model allows.
    #[test]
    fn a_well_formed_map_earns_no_clause() {
        for count in [1_usize, 2, 25, 49, 50] {
            let clauses = tag_map_clauses(&map(count));
            assert!(
                clauses.is_empty(),
                "case {count} entries: expected no clause, got {clauses:?}"
            );
        }
    }

    /// Every element of a key list is checked against the same key bounds, and each
    /// clause names the position that broke them.
    #[test]
    fn every_element_of_a_key_list_is_checked() {
        let keys = vec![
            "fine".to_string(),
            "x".repeat(129),
            "ok".to_string(),
            String::new(),
        ];
        let clauses = tag_key_list_clauses(&keys);
        let paths: Vec<&str> = clauses.iter().map(|clause| clause.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["tagKeys.2.member", "tagKeys.4.member"],
            "expected the positions that broke the key bounds, got {clauses:?}"
        );
    }
}
