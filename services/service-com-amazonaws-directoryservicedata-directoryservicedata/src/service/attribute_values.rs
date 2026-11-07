// SPDX-License-Identifier: CC0-1.0

//! Which ATTRIBUTE NAMES and ATTRIBUTE VALUES this service admits — the steps a request
//! carrying `OtherAttributes` meets after the modelled constraints the framework's own walk
//! collects and before the authorization gate ([`super::authorization`]).
//!
//! An attribute is a NAME and a VALUE, and each has a rule of its own:
//!
//! * **The NAME is a key of the `Attributes` map, and the model constrains it**: the key
//!   shape is `LdapDisplayName` (`smithy_query(query="shape", shape="Attributes")`), whose
//!   `@length` and `@pattern` a name has to satisfy exactly as every other modelled value
//!   does — so a name outside them is `ValidationException` in the constraint layer's own
//!   envelope, with no `Reason`. The framework's validating walk cannot collect that one:
//!   "Map *keys* are not covered (codegen does not capture key constraints yet)"
//!   (`vendor/smithy-schema-server/src/constraints.rs`, read-only from here), so this
//!   module collects it — as the framework's own [`Violation`], over the constraint set read
//!   off the model, so the clause, the envelope and the requirement stay where they are
//!   written and nothing about the rule is restated here.
//! * **The VALUE is an `AttributeValue` union**, whose members are `S` (a string), `N` (a
//!   number), `BOOL` and `SS` (a string set) —
//!   `smithy_query(query="shape", shape="AttributeValue")`. The union says what a value may
//!   LOOK like; which of its members this API actually takes is a rule of the service, and
//!   Directory Service Data takes the `S` member alone: an entry sent as any other member is
//!   refused with `ValidationException` (HTTP 400), the modelled `Reason`
//!   `INVALID_ATTRIBUTE_VALUE` (`ValidationExceptionReason` —
//!   `crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`) and a message naming the
//!   ATTRIBUTE, [`invalid_attribute_value`].
//!
//! The value decision is a property of the union MEMBER the request sent and of nothing
//! else: not of the attribute's name, not of the AD type that attribute holds in a
//! directory, and not of the operation that carried it. So both steps are taken here, once,
//! over the attribute map of whichever operation carries one — `CreateUser`, `UpdateUser`,
//! `CreateGroup` and `UpdateGroup` today, every operation whose input declares an
//! `Attributes` member in general — rather than inside a handler, which would also put them
//! AFTER the authorization gate the recorded ordering puts them before.
//!
//! The value refusal's grammar is not the constraint layer's: a value outside a declared
//! constraint is stated as `<N> validation error(s) detected: …` with no `Reason`
//! (`crate::service::validation`), while that step states one sentence about one attribute
//! and the `Reason` above — two different layers, two different remediations for the caller.
//!
//! The request is read as the WIRE carries it: an `AttributeValue` goes on the wire as a
//! JSON object naming the one union member it holds, so the member a request sent is the
//! key of that object, and an attribute's name is the key of the map it sits in. Nothing is
//! re-deserialized and no handler is reached, which is what lets the answers precede the
//! gate.

use std::sync::OnceLock;

use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::constraints::path;
use serde_json::Value;
use smithy_server_runtime::ViolationKind;

/// The one `AttributeValue` member this service takes a value as.
const ACCEPTED_MEMBER: &str = "S";

/// The `ValidationExceptionReason` this layer states: one of the modelled values, the one
/// that names an attribute's VALUE rather than its name or its presence.
pub const INVALID_ATTRIBUTE_VALUE: &str = "INVALID_ATTRIBUTE_VALUE";

/// The sentence this layer refuses an attribute's value with: the attribute as the request
/// spelled it, and nothing of the value — which the model marks `@sensitive` on every
/// member of the union.
#[must_use]
pub fn invalid_attribute_value(attribute: &str) -> String {
    format!("Invalid value for attribute: '{attribute}'.")
}

/// What this layer refuses a request with: which of the two rules it broke, and the prose
/// that rule is stated in.
///
/// The two are different LAYERS, not two sentences of one: a name outside the constraints
/// the model declares is the constraint layer's refusal and carries no `Reason`, while a
/// value sent as a member this API does not take is this service's own semantic refusal and
/// carries the `Reason` that names the class.
#[derive(Debug)]
pub enum AttributeRefusal {
    /// A NAME outside the modelled constraints for one: the whole constraint-layer message.
    Name(String),
    /// A VALUE sent as a member this API does not take: the sentence naming the attribute.
    Value(String),
}

impl AttributeRefusal {
    /// The message this refusal states, and the modelled `Reason` it travels with — `None`
    /// for the constraint layer's, which names no reason beyond the constraint itself.
    #[must_use]
    pub fn stated(self) -> (String, Option<&'static str>) {
        match self {
            Self::Name(message) => (message, None),
            Self::Value(message) => (message, Some(INVALID_ATTRIBUTE_VALUE)),
        }
    }
}

/// How this layer refuses `body`, or `None` when every attribute it carries is one this
/// service takes.
///
/// `route` is the called operation's input schema, off which the attribute MAP members are
/// read (`@httpQuery`-bound members and body members alike carry their modelled target's
/// kind), so nothing here holds a list of operations or of member names. The body is read
/// ONCE, for both rules: a body that is not a JSON object, or that carries no attribute map,
/// refuses nothing — reading the body into the modelled input shape is the request layer's
/// step and it has already run.
///
/// The NAME is decided first, because a name the directory could not hold cannot carry a
/// value at all.
#[must_use]
pub fn refusing_attributes(route: &Schema, body: &[u8]) -> Option<AttributeRefusal> {
    let Ok(Value::Object(members)) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let maps: Vec<(&Schema, &serde_json::Map<String, Value>)> = members
        .iter()
        .filter_map(|(key, held)| Some((attribute_map_member(route, key)?, held.as_object()?)))
        .collect();
    if let Some(message) = refusing_attribute_name(&maps) {
        return Some(AttributeRefusal::Name(message));
    }
    unaccepted_attribute(&maps)
        .map(|attribute| AttributeRefusal::Value(invalid_attribute_value(&attribute)))
}

/// The attribute among `maps` whose value this service does not take, or `None` when every
/// attribute value the request carries is one it does.
///
/// When several attributes offend, the one named is the one whose NAME sorts first — a
/// choice of this module rather than of the order a parsed object reports its entries in,
/// so one request is always refused with one same sentence however a client happened to
/// serialize it.
fn unaccepted_attribute(maps: &[(&Schema, &serde_json::Map<String, Value>)]) -> Option<String> {
    maps.iter()
        .flat_map(|(_, attributes)| attributes.iter())
        .filter(|(_, value)| !is_accepted_attribute_value(value))
        .map(|(attribute, _)| attribute.clone())
        .min()
}

/// The member of `route` a JSON key names when that member's modelled target is a MAP, the
/// shape an attribute map has.
fn attribute_map_member<'a>(route: &'a Schema, key: &str) -> Option<&'a Schema> {
    ::service_common::json_wire_values::member_by_wire_name(route, key)
        .filter(|member| matches!(member.shape_type(), ShapeType::Map))
}

/// The whole refusal message for an ATTRIBUTE NAME in `maps` that falls outside the
/// constraints the model declares for one.
///
/// The clause is the framework's, stated at the attribute MAP — a map's KEY has no member
/// path of its own, so the fault is attributed to the keys of the member the map sat at,
/// over the key shape's whole constraint set, through the tree's one statement of that
/// clause (`service_common::validation_message::map_key_refusal_quoting`). Its value half is
/// the map the request carried, rendered the way AWS echoes a map-valued member
/// ([`rendered_attributes`]): the KEY is a value the caller chose and is disclosed, while
/// each attribute's VALUE is a `@sensitive` member of the union and is replaced by the
/// marker that stands for a withheld value inside a structure.
///
/// ONE clause per attribute map, however many of its keys offend: the clause is about that
/// member's keys and the set they have to satisfy, not about the one key that failed. Where
/// an input declared two attribute maps, the clause is stated at the one whose member path
/// sorts first, so the answer is this module's choice rather than the parsed object's order.
/// `None` when every name is inside them, and `None` for a model this cannot read the key
/// shape's `@length` bounds off — the fail-safe direction, which leaves the request to the
/// layers below.
fn refusing_attribute_name(maps: &[(&Schema, &serde_json::Map<String, Value>)]) -> Option<String> {
    let rule = attribute_name_rule();
    let (min_length, max_length) = (rule.min_length?, rule.max_length?);
    maps.iter()
        .filter(|(_, attributes)| attributes.keys().any(|name| breaks_name_constraint(name)))
        .map(|(member, attributes)| {
            (
                path::member(member.member_name().unwrap_or_default()),
                *attributes,
            )
        })
        .min_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(member_path, attributes)| {
            // A name the clause quotes is a value the CALLER chose and this check is the
            // one that refuses an over-long one, so the echo is held to the ceiling one
            // clause of this tree may carry (`quoted_echo`) rather than to the request's
            // own size.
            let quoted =
                ::service_common::validation::quoted_echo(&rendered_attributes(attributes))
                    .into_owned();
            ::service_common::validation_message::map_key_refusal_quoting(
                &member_path,
                Some(&quoted),
                min_length,
                max_length,
                rule.declared_pattern
                    .as_deref()
                    // The requirement states the expression a key has to satisfy, as every
                    // other clause of this service states a `@pattern`.
                    .map(::service_common::validation::unanchored)
                    .as_deref(),
            )
        })
}

/// The marker that stands in a quoted structure for the value of a member the model marks
/// `@sensitive`: the value is withheld, and the marker says so where the value would have
/// been.
///
/// Every member of `AttributeValue` is `@sensitive`
/// (`smithy_query(query="shape", shape="AttributeValue")`), so an attribute's value is
/// never disclosed — not even inside the echo of the map it sat in, where the attribute's
/// NAME still is.
const WITHHELD_VALUE: &str = "*** REDACTED ***";

/// The shape an attribute's value is, as a quoted structure names it: the value shape of the
/// `Attributes` map (`smithy_query(query="shape", shape="Attributes")` — value
/// `AttributeValue`). A member schema carries its target's KIND and not its target's name,
/// so the name is stated here.
const ATTRIBUTE_VALUE_SHAPE: &str = "AttributeValue";

/// `attributes` as the value half of a clause about a map quotes it: `{<key>=<value>, …}` in
/// KEY order, each value the `AttributeValue` union printed over the member the request sent
/// with its value WITHHELD.
///
/// KEY order rather than the order a parsed object reports its entries in, which is the
/// order the tree's own walk renders a map's entries in: a map is unordered, so the order a
/// request wrote its entries in is not a property of the request.
///
/// The braces and the `<key>=<value>` entries are the tree's one rendering of a map
/// (`service_common::validation::rendered_map`), and a structure prints as
/// `<Shape>(<member>=<value>)` the way the AWS SDKs print one.
fn rendered_attributes(attributes: &serde_json::Map<String, Value>) -> String {
    let mut rendered: Vec<(String, String)> = attributes
        .iter()
        .map(|(name, value)| (name.clone(), withheld_union(value)))
        .collect();
    rendered.sort_by(|(left, _), (right, _)| left.cmp(right));
    let entries: Vec<(&str, &str)> = rendered
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    ::service_common::validation::rendered_map(&entries)
}

/// One attribute value printed with its value withheld: the union's shape name over the
/// member the request sent, which is the one key of the object a union goes on the wire as.
///
/// A union names exactly one member on the wire; an object naming several is printed over
/// them in member order, so the rendering is this module's choice rather than the parsed
/// object's.
fn withheld_union(value: &Value) -> String {
    let mut members: Vec<String> = value
        .as_object()
        .map(|union| {
            union
                .keys()
                .map(|member| format!("{}={WITHHELD_VALUE}", printed_member(member)))
                .collect()
        })
        .unwrap_or_default();
    members.sort();
    format!("{ATTRIBUTE_VALUE_SHAPE}({})", members.join(", "))
}

/// A member name as a printed structure spells it: the name the generated types carry for
/// it, lowercased at the front.
///
/// The SDKs print a member under the field name they generate for it, which lowercases a
/// name written entirely in capitals (`S` -> `s`, `BOOL` -> `bool`) and otherwise only the
/// first character (`SomeMember` -> `someMember`), exactly as this service's own generated
/// `AttributeValue` variants are spelled.
fn printed_member(member: &str) -> String {
    if member.chars().all(|character| !character.is_lowercase()) {
        return member.to_lowercase();
    }
    path::member(member)
}

/// Whether `name` breaks any of the constraints the model declares for an attribute name.
fn breaks_name_constraint(name: &str) -> bool {
    let rule = attribute_name_rule();
    rule.min_length.is_some_and(|min| characters(name) < min)
        || rule.max_length.is_some_and(|max| characters(name) > max)
        || rule
            .pattern
            .as_ref()
            .is_some_and(|pattern| !pattern.is_match(name))
}

/// How many characters `name` holds, which is what `@length` counts.
fn characters(name: &str) -> i64 {
    i64::try_from(name.chars().count()).unwrap_or(i64::MAX)
}

/// What an attribute NAME is held to, built ONCE from the model.
struct AttributeNameRule {
    /// The `@length` minimum the key shape declares, and its maximum; `None` for a shape
    /// that declares neither, which refuses nothing rather than inventing a bound.
    min_length: Option<i64>,
    max_length: Option<i64>,
    /// The `@pattern` that shape declares, as the MODEL writes it — anchors included, which
    /// is what the check runs.
    declared_pattern: Option<String>,
    /// That pattern compiled; `None` when the shape declares none, or when it declares one
    /// this crate's engine cannot compile — which refuses nothing rather than refusing
    /// everything.
    pattern: Option<::regex::Regex>,
}

/// The constraints an attribute NAME satisfies, read off the MODEL rather than written
/// here.
///
/// An attribute name is a key of the `Attributes` map, and codegen does not attach a map's
/// KEY constraints to the map member's schema. The key shape is the same shape the READ
/// operations carry as the ELEMENT of their `OtherAttributes` list (`LdapDisplayNameList` —
/// both target `LdapDisplayName`), and codegen DOES attach a list member's element
/// constraints to it, so the set that list declares IS the set a name must satisfy. It is
/// read through the tree's one reading of those sets
/// (`service_common::validation_message::element_constraint_set`), off two operations that
/// declare the member, which answers `None` if they ever disagreed.
fn attribute_name_rule() -> &'static AttributeNameRule {
    static RULE: OnceLock<AttributeNameRule> = OnceLock::new();
    RULE.get_or_init(|| {
        let declared = ::service_common::validation_message::element_constraint_set(
            &[
                crate::ops::types::DESCRIBE_USER_INPUT_SCHEMA,
                crate::ops::types::DESCRIBE_GROUP_INPUT_SCHEMA,
            ],
            &path::member(ATTRIBUTE_NAME_LIST_MEMBER),
        )
        .unwrap_or_default();
        let bound = |wanted: fn(&ViolationKind) -> Option<i64>| declared.iter().find_map(wanted);
        let declared_pattern = declared.iter().find_map(|kind| match kind {
            ViolationKind::Pattern(pattern) => Some(pattern.clone()),
            _ => None,
        });
        let pattern = declared_pattern
            .as_deref()
            .and_then(|pattern| ::regex::Regex::new(pattern).ok());
        AttributeNameRule {
            min_length: bound(|kind| match kind {
                ViolationKind::MinLength(min) => Some(*min),
                _ => None,
            }),
            max_length: bound(|kind| match kind {
                ViolationKind::MaxLength(max) => Some(*max),
                _ => None,
            }),
            declared_pattern,
            pattern,
        }
    })
}

/// The member whose ELEMENTS are attribute names: `OtherAttributes` on the read operations,
/// a list of the very shape an attribute map's keys are.
const ATTRIBUTE_NAME_LIST_MEMBER: &str = "OtherAttributes";

/// Whether one attribute's value is one this service takes: the `AttributeValue` union
/// naming its `S` member and NO other — exactly that one member, so an object naming none
/// at all is not a value this service takes either.
///
/// A value that is not a union on the wire at all (not a JSON object) is left alone: what
/// a value's TYPE may be is the request layer's reading, and this layer only decides
/// between the union's members.
fn is_accepted_attribute_value(value: &Value) -> bool {
    match value.as_object() {
        Some(union) => union.len() == 1 && union.contains_key(ACCEPTED_MEMBER),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{invalid_attribute_value, refusing_attributes, AttributeRefusal, INVALID_ATTRIBUTE_VALUE};

    /// RULE: the member the request sent an attribute value as decides — `S` is taken and
    /// every other member of the union is not — whatever the attribute is named and
    /// whichever operation carries the map.
    #[test]
    fn only_the_string_member_of_the_union_is_taken() {
        // (case, the called operation's input schema, the body, the attribute refused)
        let create_user = crate::ops::types::CREATE_USER_INPUT_SCHEMA;
        let update_group = crate::ops::types::UPDATE_GROUP_INPUT_SCHEMA;
        let rows: [(&str, &aws_smithy_schema::Schema, serde_json::Value, Option<&str>); 10] = [
            (
                "the S member",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {"codePage": {"S": "16"}}}),
                None,
            ),
            (
                "the N member",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {"codePage": {"N": 16}}}),
                Some("codePage"),
            ),
            (
                "the BOOL member",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {"codePage": {"BOOL": true}}}),
                Some("codePage"),
            ),
            (
                "the SS member",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {"codePage": {"SS": ["a", "b"]}}}),
                Some("codePage"),
            ),
            (
                "another attribute name, through another operation",
                update_group,
                json!({"SAMAccountName": "g", "OtherAttributes": {"description": {"N": 1}}}),
                Some("description"),
            ),
            (
                "one offender among taken attributes",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {
                    "aa": {"S": "x"}, "bb": {"BOOL": false}, "cc": {"S": "y"}
                }}),
                Some("bb"),
            ),
            (
                "several offenders: the one whose name sorts first",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {
                    "zz": {"N": 1}, "aa": {"SS": []}
                }}),
                Some("aa"),
            ),
            (
                "no attribute map at all",
                create_user,
                json!({"SAMAccountName": "u"}),
                None,
            ),
            (
                "an attribute map carrying nothing",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {}}),
                None,
            ),
            (
                "a union naming no member at all, which is not the S member",
                create_user,
                json!({"SAMAccountName": "u", "OtherAttributes": {"codePage": {}}}),
                Some("codePage"),
            ),
        ];
        for (case, route, body, expected) in rows {
            let refused = refusing_attributes(route, body.to_string().as_bytes());
            let stated = match refused {
                Some(AttributeRefusal::Value(message)) => {
                    let (message, reason) = AttributeRefusal::Value(message).stated();
                    assert_eq!(
                        reason,
                        Some(INVALID_ATTRIBUTE_VALUE),
                        "case {case:?}: a value refusal states the Reason that names it"
                    );
                    Some(message)
                }
                Some(AttributeRefusal::Name(message)) => panic!(
                    "case {case:?}: every name here is inside its constraints, got {message:?}"
                ),
                None => None,
            };
            let expected = expected.map(invalid_attribute_value);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// The sentence names the attribute the request spelled and no value.
    #[test]
    fn the_sentence_names_the_attribute_alone() {
        let stated = invalid_attribute_value("codePage");
        assert_eq!(
            stated, "Invalid value for attribute: 'codePage'.",
            "expected the attribute named and nothing else, got {stated:?}"
        );
    }
}
