// SPDX-License-Identifier: CC0-1.0

//! The ORDER a constraint refusal lists an enumerated type's admissible values in.
//!
//! `Member must satisfy enum value set: [ … ]` states WHICH values the type admits,
//! and the frontend that words it enumerates them out of a hash table keyed on the
//! value's own text rather than walking the type's declaration: the order is
//! therefore neither the model's declaration order nor the sorted one, it is a
//! property of the VALUES alone, and it is the same for every member, operation and
//! request that reaches one enumerated type.
//!
//! The table this reproduces, read off the refusals this service answers:
//!
//! * [`BUCKETS`] buckets, indexed by the low bits of the value's string hash —
//!   the 31-polynomial over the value's UTF-16 code units ([`string_hash`]);
//! * the buckets enumerated in ascending index order;
//! * a bucket holding more than one value (two values whose hashes agree in those
//!   low bits) enumerated with the LATER-declared value first, the chain order of a
//!   table whose entries are pushed onto the head of their bucket.
//!
//! Keying on the values means an enumerated type no refusal has been seen stating is
//! listed by the same rule as one that has, and a model revision that reorders a
//! type's declaration does not move its stated set — only adding or removing a value
//! does.

/// How many buckets the admissible set is enumerated out of.
///
/// One width fits every set this service's refusals are known to state — they run
/// from two values up to eight — and the width belongs to the table rather than to
/// the set, so a wider set is enumerated out of the same buckets, more of which then
/// hold a chain.
const BUCKETS: usize = 32;

/// `declared` — an enumerated type's admissible values, in the order the model
/// declares them — in the order this service enumerates them in (see the module
/// docs).
pub(super) fn as_enumerated<S: AsRef<str>>(declared: &[S]) -> Vec<String> {
    let mut buckets: Vec<Vec<&str>> = vec![Vec::new(); BUCKETS];
    for value in declared {
        let value = value.as_ref();
        buckets[bucket_of(value)].push(value);
    }
    buckets
        .into_iter()
        // A chain is read from its head, onto which each value was pushed, so a
        // bucket states the LATER-declared value first.
        .flat_map(|bucket| bucket.into_iter().rev())
        .map(str::to_string)
        .collect()
}

/// The bucket `value` falls in: the low bits of its hash, which for a power-of-two
/// width is the hash modulo that width — in range by construction.
fn bucket_of(value: &str) -> usize {
    (string_hash(value) as u32 as usize) % BUCKETS
}

/// The hash of `value`: the 31-polynomial over its UTF-16 code units, accumulated in
/// 32 bits with wrapping arithmetic — the hash the string carries where this
/// enumeration order comes from, and the one the bucket index is read out of.
fn string_hash(value: &str) -> i32 {
    value.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A set is RE-ORDERED, never changed: every value the model declares is stated
    /// exactly once, whatever the set's size. The widest type this model declares
    /// (`IndustrySegment`, 37 values) has more values than the table has buckets, so
    /// several of its buckets hold a chain.
    #[test]
    fn every_declared_value_is_stated_exactly_once() {
        for declared in [
            crate::ops::types::INDUSTRY_SEGMENT_VALUES,
            crate::ops::types::PRIMARY_SOLUTION_TYPE_VALUES,
            crate::ops::types::INVITATION_STATUS_VALUES,
            crate::ops::types::CONNECTION_TYPE_VALUES,
            crate::ops::types::PROFILE_VISIBILITY_VALUES,
        ] {
            let stated = as_enumerated(declared);
            let mut sorted_stated = stated.clone();
            sorted_stated.sort_unstable();
            let mut sorted_declared: Vec<String> =
                declared.iter().map(|value| (*value).to_string()).collect();
            sorted_declared.sort_unstable();
            assert_eq!(
                sorted_stated, sorted_declared,
                "case {declared:?}: the stated set is the declared one re-ordered, got {stated:?}"
            );
        }
    }

    /// The two enumerated types this service's refusals are known to state a set for,
    /// each held to the order those refusals state it in — neither the declaration
    /// order the rows list nor the sorted one.
    #[test]
    fn a_known_set_is_stated_in_the_order_the_refusals_state_it() {
        // (type, declared order, the order a refusal states it in)
        let rows: &[(&str, &[&str], &[&str])] = &[
            (
                // `smithy_query(query="shape",shape="PrimarySolutionType")`.
                "PrimarySolutionType",
                &[
                    "SOFTWARE_PRODUCTS",
                    "CONSULTING_SERVICES",
                    "PROFESSIONAL_SERVICES",
                    "MANAGED_SERVICES",
                    "HARDWARE_PRODUCTS",
                    "COMMUNICATION_SERVICES",
                    "VALUE_ADDED_RESALE_AWS_SERVICES",
                    "TRAINING_SERVICES",
                ],
                &[
                    "TRAINING_SERVICES",
                    "COMMUNICATION_SERVICES",
                    "CONSULTING_SERVICES",
                    "PROFESSIONAL_SERVICES",
                    "VALUE_ADDED_RESALE_AWS_SERVICES",
                    "HARDWARE_PRODUCTS",
                    "SOFTWARE_PRODUCTS",
                    "MANAGED_SERVICES",
                ],
            ),
            (
                // `smithy_query(query="shape",shape="InvitationStatus")`.
                "InvitationStatus",
                &["PENDING", "ACCEPTED", "REJECTED", "CANCELED", "EXPIRED"],
                &["EXPIRED", "ACCEPTED", "PENDING", "CANCELED", "REJECTED"],
            ),
        ];

        for (case, declared, stated) in rows {
            assert_eq!(
                as_enumerated(declared),
                *stated,
                "case {case:?}: the set is stated in the order the refusals state it in, got {:?}",
                as_enumerated(declared)
            );
        }
    }
}
