// SPDX-License-Identifier: CC0-1.0

//! What an expression's OPERANDS may be: the closed domains the functions and the
//! grammar hold a literal operand to.
//!
//! These are facts of the TEXT one request carries, so they are settled where the
//! expression is read rather than where an item is compared: an operand outside its
//! domain is the caller's mistake whatever the addressed table holds, and a request
//! carrying one is refused before anything is read. [`super`] re-exports them, so every
//! caller still names them `crate::state::…`.

use super::*;

/// Whether a bare word begins with a digit, which is what tells a numeric literal from
/// an attribute name written out in an expression ([`ExprParser::path_name`]).
pub(super) fn starts_with_digit(word: &str) -> bool {
    word.starts_with(|character: char| character.is_ascii_digit())
}

/// `begins_with`'s prefix operand is a PREFIX, so it is a string or binary value, and a
/// literal of any other type is refused when the expression is read.
///
/// "begins_with (path, substr) — true if the attribute specified by path begins with a
/// particular substring" (the `ConditionExpression` doc): only `S` and `B` have prefixes
/// to compare, so a number or a collection cannot be one, and the operand is a literal
/// the request already carried — the expression is refused whether or not the addressed
/// table holds an item to evaluate it against. An operand that reads an attribute is
/// left to the evaluation, which is the only place its type is known.
pub(super) fn check_begins_with_prefix(
    member: &str,
    operand: &Operand,
) -> Result<(), RequestError> {
    let Operand::Value(value) = operand else {
        return Ok(());
    };
    if matches!(value, AttributeValue::S(_) | AttributeValue::B(_)) {
        return Ok(());
    }
    Err(incorrect_operand_type(
        member,
        "begins_with",
        attribute_value_type(value),
    ))
}

/// The type codes a type test may name, in the order AWS's own refusal lists them.
///
/// Source: this service's Smithy model, the members of the `AttributeValue` union
/// (`smithy_query(query="shape",shape="AttributeValue")`) — the codes a stored value can
/// carry are exactly the codes a test of its type can name.
pub const ATTRIBUTE_TYPE_NAMES: [&str; 10] =
    ["S", "SS", "N", "NS", "B", "BS", "BOOL", "NULL", "L", "M"];

/// `attribute_type`'s second operand names a TYPE, so it is held to that closed set as
/// the expression is READ rather than as an item is compared.
///
/// The operand is a literal the request already carried, and a type test naming no type
/// can select nothing, so the request is refused whether or not the addressed table
/// holds an item to evaluate it against — a token that is no type code is the caller's
/// mistake, not an empty result. A code the set does not hold is named back as the
/// request spelled it, beside the whole set; a literal that is not a string at all is
/// the operand-type refusal, since the function takes a type NAME. An operand that
/// reads an attribute instead of naming a literal is left to the evaluation, which is
/// the only place its type is known.
pub(super) fn check_attribute_type_name(
    member: &str,
    operand: &Operand,
) -> Result<(), RequestError> {
    let Operand::Value(value) = operand else {
        return Ok(());
    };
    let AttributeValue::S(name) = value else {
        return Err(incorrect_operand_type(
            member,
            "attribute_type",
            attribute_value_type(value),
        ));
    };
    if ATTRIBUTE_TYPE_NAMES.contains(&name.as_str()) {
        return Ok(());
    }
    Err(invalid_expression_member(
        member,
        &format!(
            "Invalid attribute type name found; type: {name}, valid types: {{{}}}",
            ATTRIBUTE_TYPE_NAMES.join(",")
        ),
    ))
}
