// SPDX-License-Identifier: CC0-1.0

//! Comparing a stored field value against one of the model's `FieldFilter`
//! comparisons.
//!
//! `FieldFilter` is reached from more than one search — `CaseFilter$field`
//! narrows a search of cases with it, `CustomFieldsFilter$field` narrows a
//! search of related items with it — and both ask the same question of a stored
//! value, so the comparisons live beside the store rather than in one
//! operation's file.

use super::*;

use crate::ops::types::{FieldFilter, FieldValue};

/// The `FieldValue` a `FieldFilter` compares against: every member of that
/// union targets `FieldValue`
/// (`smithy_query(query="shape", shape="FieldFilter")`), which carries both the
/// field id the filter is about and the value to compare with.
pub fn field_filter_operand(filter: &FieldFilter) -> &FieldValue {
    match filter {
        FieldFilter::equalTo(operand)
        | FieldFilter::Contains(operand)
        | FieldFilter::greaterThan(operand)
        | FieldFilter::greaterThanOrEqualTo(operand)
        | FieldFilter::lessThan(operand)
        | FieldFilter::lessThanOrEqualTo(operand) => operand,
    }
}

/// The order one of AWS's comparisons relates two field values by, or `None`
/// when none of them does: values of two different union variants are values of
/// two different types, and nothing orders them, so no ordered `FieldFilter`
/// may admit such a pair and no `equalTo` may call it equal. A double that is
/// ordered against nothing (a NaN) is likewise related to nothing, which is
/// what `partial_cmp` answers for it. `Text`, `Url`, `SingleSelect` and
/// `DateTime` values are all carried as `stringValue`, and lexicographic order
/// over an RFC 3339 instant is chronological order.
pub fn compared_field_values(
    left: &FieldValueUnion,
    right: &FieldValueUnion,
) -> Option<::std::cmp::Ordering> {
    match (left, right) {
        (FieldValueUnion::stringValue(left), FieldValueUnion::stringValue(right))
        | (FieldValueUnion::userArnValue(left), FieldValueUnion::userArnValue(right)) => {
            Some(left.cmp(right))
        }
        (FieldValueUnion::booleanValue(left), FieldValueUnion::booleanValue(right)) => {
            Some(left.cmp(right))
        }
        (FieldValueUnion::doubleValue(left), FieldValueUnion::doubleValue(right)) => {
            left.partial_cmp(right)
        }
        (FieldValueUnion::emptyValue(_), FieldValueUnion::emptyValue(_)) => {
            Some(::std::cmp::Ordering::Equal)
        }
        _ => None,
    }
}

/// The text a value carries, for the containment comparisons, or `None` when it
/// carries none: only the string-valued members of the union hold text.
pub fn field_value_text(value: &FieldValueUnion) -> Option<&str> {
    match value {
        FieldValueUnion::stringValue(text) | FieldValueUnion::userArnValue(text) => Some(text),
        _ => None,
    }
}

/// Does the value held for the filtered field satisfy the comparison?
///
/// `stored` is what the resource holds for that field, `None` when it holds
/// nothing — which is also what a stored `emptyValue` means, since that is the
/// union's own empty member and exactly what a field that was never written
/// reads back as.
pub fn field_filter_matches(stored: Option<&FieldValueUnion>, filter: &FieldFilter) -> bool {
    use ::std::cmp::Ordering;

    match filter {
        // Equality is equality of the value, variant included. Asking for the
        // union's `emptyValue` asks for the resources that hold no value for the
        // field, since that is how an unwritten field reads back.
        FieldFilter::equalTo(operand) => match (stored, &operand.value) {
            (stored, FieldValueUnion::emptyValue(_)) => stored.is_none(),
            (None, _) => false,
            (Some(stored), wanted) => {
                compared_field_values(stored, wanted) == Some(Ordering::Equal)
            }
        },
        // Containment is over text, compared without regard to case, and a value
        // that carries no text contains nothing.
        FieldFilter::Contains(operand) => {
            match (
                stored.and_then(field_value_text),
                field_value_text(&operand.value),
            ) {
                (Some(stored), Some(wanted)) => {
                    stored.to_lowercase().contains(&wanted.to_lowercase())
                }
                _ => false,
            }
        }
        FieldFilter::greaterThan(operand) => ordered_as(stored, operand, &[Ordering::Greater]),
        FieldFilter::greaterThanOrEqualTo(operand) => {
            ordered_as(stored, operand, &[Ordering::Greater, Ordering::Equal])
        }
        FieldFilter::lessThan(operand) => ordered_as(stored, operand, &[Ordering::Less]),
        FieldFilter::lessThanOrEqualTo(operand) => {
            ordered_as(stored, operand, &[Ordering::Less, Ordering::Equal])
        }
    }
}

/// Does the stored value stand in one of `accepted` orderings to the filter's
/// value? A field with no value, and a value no order relates to the filter's,
/// satisfies no ordered comparison.
fn ordered_as(
    stored: Option<&FieldValueUnion>,
    operand: &FieldValue,
    accepted: &[::std::cmp::Ordering],
) -> bool {
    stored
        .and_then(|stored| compared_field_values(stored, &operand.value))
        .is_some_and(|ordering| accepted.contains(&ordering))
}

/// One `FieldFilter` clause rendered so that two requests carrying the same
/// clause render the same text and two carrying different clauses do not: the
/// field it compares, the comparison it names and the value it compares with.
/// What the searches build a paging scope out of, so a continuation token
/// belongs to the criteria it was issued for.
pub fn field_filter_fingerprint(filter: &FieldFilter) -> String {
    let operand = field_filter_operand(filter);
    format!(
        "field({} {} {})",
        operand.id,
        comparison_name(filter),
        field_value_fingerprint(&operand.value)
    )
}

/// Which comparison a `FieldFilter` names.
fn comparison_name(filter: &FieldFilter) -> &'static str {
    match filter {
        FieldFilter::equalTo(_) => "equalTo",
        FieldFilter::Contains(_) => "contains",
        FieldFilter::greaterThan(_) => "greaterThan",
        FieldFilter::greaterThanOrEqualTo(_) => "greaterThanOrEqualTo",
        FieldFilter::lessThan(_) => "lessThan",
        FieldFilter::lessThanOrEqualTo(_) => "lessThanOrEqualTo",
    }
}

/// One field value rendered for [`field_filter_fingerprint`]: its union variant
/// and the content it carries, so two values that differ in either render
/// differently.
fn field_value_fingerprint(value: &FieldValueUnion) -> String {
    let content = match value {
        FieldValueUnion::stringValue(text) | FieldValueUnion::userArnValue(text) => text.clone(),
        FieldValueUnion::doubleValue(number) => number.to_string(),
        FieldValueUnion::booleanValue(flag) => flag.to_string(),
        FieldValueUnion::emptyValue(_) => String::new(),
    };
    format!("{}:{content}", variant_name(value))
}
