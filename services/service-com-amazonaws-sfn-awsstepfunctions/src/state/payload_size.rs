// SPDX-License-Identifier: CC0-1.0

//! The members whose `@length` bounds a PAYLOAD, which this service measures and
//! states in UTF-8 BYTES.
//!
//! Most constrained members of this model are bounded in characters, which is what
//! Smithy's `@length` counts on a string and what the framework's constraint walk
//! enforces. The members that carry execution DATA are different, and the model says
//! so in their own documentation: "Length constraints apply to the payload size, and
//! are expressed as bytes in UTF-8 encoding"
//! (`smithy_query(query="doc",shape="StartExecutionInput",member="input")`). For those
//! members the bound is a size in bytes, so:
//!
//! * a value whose UTF-8 encoding is longer than the bound is refused even where it
//!   carries fewer characters than that — the walk's character count cannot see it, so
//!   the handler that takes the member enforces it
//!   ([`over_payload_limit`]); and
//! * the requirement a refusal states is the one about that size
//!   ([`BYTE_SIZE_REQUIREMENT`]), not the character-length sentence the framework
//!   states for every other member.
//!
//! WHICH members those are is read off this service's MODEL by that documented
//! sentence, exactly as which members a refusal withholds the value of is read off the
//! `@sensitive` trait: nothing here lists a member name, so the rule cannot fall behind
//! a model that documents the sentence on one more member. The bound comes from the
//! same place — the `@length` maximum the member's target shape declares — so the
//! number a refusal states and the number it checked are one value.

use ::std::collections::BTreeMap;
use ::std::sync::LazyLock;

use ::serde_json::Value;

/// This service's Smithy model: the one source for which of its members are bounded
/// by a payload SIZE, and for the bound each of them carries.
const MODEL_JSON: &str = include_str!("../../smithy-model/com-amazonaws-sfn-awsstepfunctions.json");

/// The members of this model whose `@length` maximum is a size in UTF-8 bytes, read
/// off [`MODEL_JSON`] rather than listed here.
///
/// This model documents the sentence on the data a caller submits for a run: an
/// execution's `input` (`StartExecution`, `StartSyncExecution`) and a task's `output`
/// (`SendTaskSuccess`).
pub static PAYLOAD_MEMBERS: LazyLock<PayloadMembers> =
    LazyLock::new(|| PayloadMembers::from_model(MODEL_JSON));

/// The sentence a member's own documentation carries when its `@length` bounds a
/// payload SIZE in UTF-8 bytes rather than a count of characters.
const BYTES_IN_UTF8_SENTENCE: &str = "expressed as bytes in UTF-8 encoding";

/// The Smithy trait carrying a shape's or a member's documentation.
const DOCUMENTATION_TRAIT: &str = "smithy.api#documentation";

/// The Smithy trait carrying a shape's or a member's `@length` bounds.
const LENGTH_TRAIT: &str = "smithy.api#length";

/// The members of one service's model whose `@length` maximum is a size in UTF-8
/// bytes, each with that maximum.
///
/// Built once from the model text (a compile-time constant, so the answer cannot
/// change while the process runs) and asked per request.
#[derive(Debug, Clone, Default)]
pub struct PayloadMembers {
    /// Each such member, by the name a request path spells it with, against the
    /// maximum number of UTF-8 bytes its value may hold.
    limits: BTreeMap<String, usize>,
}

impl PayloadMembers {
    /// The payload members `model_json` declares, by the rule in the module header.
    ///
    /// A model that cannot be read yields an empty set, which measures nothing in
    /// bytes and states no requirement of its own — the fail-safe direction, since
    /// that leaves the framework's own character-length wording exactly as it is.
    #[must_use]
    pub fn from_model(model_json: &str) -> Self {
        let model: Value = ::serde_json::from_str(model_json).unwrap_or(Value::Null);
        let Some(shapes) = model.get("shapes").and_then(Value::as_object) else {
            return Self::default();
        };
        let mut limits: BTreeMap<String, usize> = BTreeMap::new();
        for shape in shapes.values() {
            let Some(members) = shape.get("members").and_then(Value::as_object) else {
                continue;
            };
            for (name, member) in members {
                if !documents_byte_size(member) {
                    continue;
                }
                let Some(max) = length_maximum(member, shapes) else {
                    continue;
                };
                limits.insert(name.clone(), max);
            }
        }
        Self { limits }
    }

    /// The maximum number of UTF-8 bytes the member a refusal reports at
    /// `member_path` may carry, or `None` when that member's bound is not a size in
    /// bytes.
    ///
    /// The LEAF of the path names the member, which is how the framework's own walk
    /// reports one: a payload member nested inside a structure the caller sent is
    /// still that member.
    #[must_use]
    pub fn limit_at(&self, member_path: &str) -> Option<usize> {
        let member = member_path.split('.').next_back()?;
        self.limits.get(member).copied()
    }

    /// The payload members and their maxima, for this service's own test to pin
    /// against its model.
    #[must_use]
    pub fn limits(&self) -> &BTreeMap<String, usize> {
        &self.limits
    }
}

/// Whether `member` documents its `@length` as a payload size in UTF-8 bytes.
fn documents_byte_size(member: &Value) -> bool {
    member
        .get("traits")
        .and_then(Value::as_object)
        .and_then(|traits| traits.get(DOCUMENTATION_TRAIT))
        .and_then(Value::as_str)
        .is_some_and(|documentation| documentation.contains(BYTES_IN_UTF8_SENTENCE))
}

/// The `@length` maximum in force on `member`: the one the member itself declares,
/// else the one the shape it targets declares.
fn length_maximum(member: &Value, shapes: &::serde_json::Map<String, Value>) -> Option<usize> {
    let declared = |node: &Value| {
        node.get("traits")
            .and_then(Value::as_object)
            .and_then(|traits| traits.get(LENGTH_TRAIT))
            .and_then(|length| length.get("max"))
            .and_then(Value::as_u64)
            .and_then(|max| usize::try_from(max).ok())
    };
    if let Some(max) = declared(member) {
        return Some(max);
    }
    let target = member.get("target").and_then(Value::as_str)?;
    declared(shapes.get(target)?)
}

/// The requirement a refusal states for a value past a payload member's bound: the
/// SIZE that member may hold, in UTF-8 bytes.
///
/// RECORDED text, and the sentence AWS states for every member documented that way,
/// so it is built here from the bound the model declares rather than written at a call
/// site.
#[must_use]
pub fn byte_size_requirement(max_bytes: usize) -> String {
    format!("Member must have size less than or equal to {max_bytes} bytes in UTF-8 encoding")
}

/// Whether `value` is past the payload bound of the member at `member_path`, measured
/// in UTF-8 bytes.
///
/// `None` for a member whose bound is not a size in bytes, and for a value inside its
/// bound. `Some(max)` carries the maximum the value broke, which is what a refusal
/// states.
#[must_use]
pub fn over_payload_limit(members: &PayloadMembers, member_path: &str, value: &str) -> Option<usize> {
    let max = members.limit_at(member_path)?;
    (value.len() > max).then_some(max)
}

/// The WHOLE message a refusal of `value` at `member_path` states, or `None` when the
/// value is inside that member's payload bound — or when the member carries no such
/// bound.
///
/// This is the half of the rule the centralized constraint walk cannot enforce: the
/// walk counts a string's CHARACTERS, so a payload whose UTF-8 encoding is past the
/// bound while its character count is not reaches the handler, and the handler that
/// takes the member refuses it here. The message is built through the service's ONE
/// validation envelope ([`crate::service::validation::one_violation_message`]), so a
/// refusal the walk states and one a handler states are the same sentence — member
/// path, value half (withheld for a `@sensitive` member) and requirement alike.
#[must_use]
pub fn payload_refusal(member_path: &str, value: &str) -> Option<String> {
    let max = over_payload_limit(&PAYLOAD_MEMBERS, member_path, value)?;
    Some(crate::service::validation::one_violation_message(
        ::aws_smithy_schema_server::Violation {
            path: member_path.to_string(),
            kind: ::aws_smithy_schema_server::ViolationKind::MaxLength(
                i64::try_from(max).unwrap_or(i64::MAX),
            ),
            // The value half is decided by the model, through the same withholding
            // the walk's own refusals go through, so nothing of a `@sensitive`
            // payload is carried into the message from here.
            value: None,
            sensitive: true,
        },
    ))
}
