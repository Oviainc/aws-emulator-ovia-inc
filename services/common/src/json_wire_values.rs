// SPDX-License-Identifier: CC0-1.0

//! The CLASS of a JSON value a request carries, the member a JSON key names, and how a
//! service words a value the modelled member it is bound to cannot be read from.
//!
//! Three services put a request-DESERIALIZATION layer in front of their application code,
//! because the framework renders a codec failure in the same envelope as a constraint
//! violation while AWS answers the two as different error classes
//! (`SerializationException` for a value that never became a value, `ValidationException`
//! for one that was read and then broke a rule). Each layer reaches that decision its own
//! way — one walks the body against the input schema, one runs the framework's own
//! deserializer and watches which read failed, one reads only the members the protocol
//! carries in a fixed form — but all three then have to say the SAME two things: which
//! class of JSON value arrived, and what it could not be read into. Those two live here,
//! so the sentence a caller is refused with is written once.
//!
//! The reverse direction lives here too ([`value_text`]): the TEXT a member whose wire form
//! is a string carries for a value a caller wrote as a number, a flag or a document.

use aws_smithy_schema::{Schema, ShapeType};
use serde_json::Value;

/// The names a refusal states a JSON value's class under — the vocabulary of every such
/// refusal in this tree, whether the class was read off the VALUE ([`json_value_class`]) or
/// out of the codec's own complaint about it.
pub const NULL_VALUE: &str = "NULL_VALUE";
/// See [`NULL_VALUE`].
pub const BOOLEAN_VALUE: &str = "BOOLEAN_VALUE";
/// See [`NULL_VALUE`].
pub const NUMBER_VALUE: &str = "NUMBER_VALUE";
/// See [`NULL_VALUE`].
pub const STRING_VALUE: &str = "STRING_VALUE";
/// See [`NULL_VALUE`].
pub const ARRAY_VALUE: &str = "ARRAY_VALUE";
/// See [`NULL_VALUE`].
pub const OBJECT_VALUE: &str = "OBJECT_VALUE";

/// The class of JSON value this is, as a coercion failure names it: `<JSON type>_VALUE`.
///
/// The refusal names the CLASS, never the value: the recorded refusal of a string bound to
/// an integer member is "STRING_VALUE can not be converted to an Integer" whatever string
/// was sent, which is both what the service says and what keeps a caller from amplifying a
/// long value into a long response. That one recorded name fixes the form; the rest follow
/// it.
#[must_use]
pub fn json_value_class(value: &Value) -> &'static str {
    match value {
        Value::Null => NULL_VALUE,
        Value::Bool(_) => BOOLEAN_VALUE,
        Value::Number(_) => NUMBER_VALUE,
        Value::String(_) => STRING_VALUE,
        Value::Array(_) => ARRAY_VALUE,
        Value::Object(_) => OBJECT_VALUE,
    }
}

/// One JSON value as the TEXT a member that carries it as a string holds: a string stays
/// itself, and every other form is its own compact JSON (`30`, `true`,
/// `{"maxReceiveCount":3}`).
///
/// Several services take a value as text on the wire while the same value is TYPED wherever
/// a model declares it — an Amazon SQS queue attribute is a string while the
/// `AWS::SQS::Queue` property that carries it is a number, a boolean or an object, and a
/// template may declare any such member as the document it is. This is the one conversion
/// between the two, so two services cannot disagree about what a number or an object spells
/// as.
#[must_use]
pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The message a failed coercion states: the CLASS of value the request sent and the
/// modelled type it could not be read as, which are the two facts a caller has to
/// reconcile. The type is named by its Smithy shape type (`Integer`, `Long`, `Boolean`, …);
/// the article is grammatical.
///
/// `class` is one of [`json_value_class`]'s names — or, for a layer that learns the class
/// from the codec's own refusal rather than from the value, the same name read out of that
/// refusal.
#[must_use]
pub fn coercion_message(class: &str, target: ShapeType) -> String {
    let named = format!("{target:?}");
    unreadable_message(class, &format!("{} {named}", article_for(&named)))
}

/// The same sentence for a target the SHAPE TYPE does not name on its own: a form the
/// protocol carries a value in, stated as the thing it could not be read into (an
/// `epoch-seconds` timestamp is "milliseconds since epoch", not "a Timestamp").
///
/// One sentence shape for both, so a service refusing a value it could not read states it
/// the way every other one does whether the target is a modelled type or a wire form.
#[must_use]
pub fn unreadable_message(class: &str, target: &str) -> String {
    format!("{class} can not be converted to {target}")
}

/// The indefinite article that goes in front of a type's name.
fn article_for(named: &str) -> &'static str {
    if named.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    }
}

/// The member of `schema` a JSON key names: its `@jsonName` when the model gives one, else
/// its Smithy member name — the same lookup the JSON codec performs.
#[must_use]
pub fn member_by_wire_name<'a>(schema: &'a Schema, key: &str) -> Option<&'a Schema> {
    schema.members().iter().copied().find(|member| {
        let wire = member
            .json_name()
            .map(|name| name.value())
            .or_else(|| member.member_name())
            .unwrap_or_default();
        wire == key
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every JSON class has the name a refusal states it under, and the name is of the
    /// CLASS rather than of the value.
    #[test]
    fn every_json_class_is_named_by_its_type() {
        // (the value, the class a refusal names it)
        let cases = [
            (json!(null), "NULL_VALUE"),
            (json!(true), "BOOLEAN_VALUE"),
            (json!(7), "NUMBER_VALUE"),
            (json!(7.5), "NUMBER_VALUE"),
            (json!("7"), "STRING_VALUE"),
            (json!("a much longer value"), "STRING_VALUE"),
            (json!([7]), "ARRAY_VALUE"),
            (json!({"seconds": 7}), "OBJECT_VALUE"),
        ];
        for (value, expected) in cases {
            assert_eq!(
                json_value_class(&value),
                expected,
                "case {value}: expected the class name, got {}",
                json_value_class(&value)
            );
        }
    }

    /// The sentence names the class, then what it could not be read into — with the
    /// grammatical article for a modelled type, and verbatim for a wire form.
    #[test]
    fn the_refusal_states_the_class_and_the_target() {
        // (class, target shape type, the sentence)
        let cases = [
            (
                "STRING_VALUE",
                ShapeType::Integer,
                "STRING_VALUE can not be converted to an Integer",
            ),
            (
                "STRING_VALUE",
                ShapeType::Long,
                "STRING_VALUE can not be converted to a Long",
            ),
            (
                "OBJECT_VALUE",
                ShapeType::Boolean,
                "OBJECT_VALUE can not be converted to a Boolean",
            ),
        ];
        for (class, target, expected) in cases {
            let stated = coercion_message(class, target);
            assert_eq!(
                stated, expected,
                "case {class}/{target:?}: expected {expected:?}, got {stated:?}"
            );
        }
        assert_eq!(
            unreadable_message("STRING_VALUE", "milliseconds since epoch"),
            "STRING_VALUE can not be converted to milliseconds since epoch",
            "a wire form is stated as submitted, with no article added"
        );
    }

    /// A value a member carries as TEXT keeps a string as it is and renders every other
    /// form as its own compact JSON.
    #[test]
    fn a_typed_value_is_carried_as_the_text_it_spells() {
        // (the value, the text a member that carries it as a string holds)
        let cases = [
            (json!("already text"), "already text"),
            (json!(""), ""),
            (json!(30), "30"),
            (json!(-1), "-1"),
            (json!(1.5), "1.5"),
            (json!(true), "true"),
            (json!(false), "false"),
            (json!(null), "null"),
            (json!([1, "two"]), "[1,\"two\"]"),
            (
                json!({"deadLetterTargetArn": "arn", "maxReceiveCount": 3}),
                "{\"deadLetterTargetArn\":\"arn\",\"maxReceiveCount\":3}",
            ),
        ];
        for (value, expected) in cases {
            let carried = value_text(&value);
            assert_eq!(
                carried, expected,
                "case {value}: expected the text {expected:?}, got {carried:?}"
            );
        }
    }
}
