// SPDX-License-Identifier: CC0-1.0

//! What a VECTOR is: the one reading of the attribute a vector index indexes items by.
//!
//! "Each element is a 32-bit IEEE-754 floating point number, provided in DynamoDB list
//! format" (`SearchVectors$SearchVector`, which is the same shape
//! `VectorIndex$VectorAttribute` names on an item), so a vector is a LIST whose every
//! element is a number such a float can hold. Three rules are decided by that reading —
//! which items a write may store, which items an index holds an entry for, and what a
//! search compares — so it is stated once here rather than once per caller. It is
//! re-exported up to `crate::state`, so every caller names it `crate::state::…`.

use crate::ops::types::AttributeValue;

/// The numbers `value` holds as a vector, or `None` when it is not one: a value of
/// another type, or a list carrying an element no 32-bit float can hold.
///
/// The length is the caller's to judge — an index requires exactly its `Dimensions`,
/// while a search vector is compared against the index it names — so this answers only
/// what the value IS.
pub fn vector_values(value: &AttributeValue) -> Option<Vec<f32>> {
    let AttributeValue::L(values) = value else {
        return None;
    };
    values
        .iter()
        .map(|element| match element {
            AttributeValue::N(text) => text
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite()),
            _ => None,
        })
        .collect()
}
