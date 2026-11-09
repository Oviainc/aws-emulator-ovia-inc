// SPDX-License-Identifier: CC0-1.0

//! WHICH of the stated value-set orders this service takes, and the evidence for it.
//!
//! Documentation and evidence only — the renderer itself is
//! `service_common::validation_message`'s, so a normal build compiles no code from here.
//!
//! A constraint clause about a member of an enumerated type states the values that type
//! admits (`Member must satisfy enum value set: [<values>]`), and a front door is seen
//! listing them in one of three orders: lexicographically, as a set the service publishes,
//! or in the order the hash table the values are held in is walked. All three live in
//! `service_common::validation_message`; this service's refusals show the third
//! ([`::service_common::validation_message::in_hashed_enum_order`]) — the values' own
//! buckets ascending, a shared bucket read from the head each value was pushed onto — so
//! that is the one its refusal seam hands every violation through
//! ([`crate::service`]'s `constraint_clause`).
//!
//! The order is computed from the declared values themselves, so it belongs to the
//! enumerated TYPE: one type is stated alike under every member targeting it — any
//! operation, any depth, any list index — and a type this model gains needs nothing added
//! here. It is the type's up to the chain order of values that COLLIDE, which follows
//! their declaration order. The evidence that it is this service's order is below: the set
//! its refusal of a processor type outside `ProcessorType` states.

/// The order this service's refusal states `ProcessorType`'s value set in.
///
/// Held here as the EVIDENCE for the order chosen above, not as a table the renderer
/// reads: the renderer derives the listing from the values, and this is the one listing
/// a refusal of this service is known to state, which that derivation has to reproduce.
#[cfg(test)]
const PROCESSOR_TYPE_VALUE_SET: [&str; 6] = [
    "Decompression",
    "Lambda",
    "RecordDeAggregation",
    "MetadataExtraction",
    "AppendDelimiterToRecord",
    "CloudWatchLogProcessing",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared, value-derived order reproduces the set this service's refusal states —
    /// which is neither the model's declaration order nor a sort of the values, so taking
    /// it is a choice this pin holds the renderer to.
    #[test]
    fn the_hashed_order_reproduces_the_set_this_service_states() {
        // `smithy_query(query="shape", shape="ProcessorType")` declares the values in this
        // order; `crate::ops::types::shared_types::PROCESSOR_TYPE_VALUES` holds the same
        // six sorted, and the assertions below hold the stated listing apart from both.
        let declared = [
            "RecordDeAggregation",
            "Decompression",
            "CloudWatchLogProcessing",
            "Lambda",
            "MetadataExtraction",
            "AppendDelimiterToRecord",
        ];
        let stated = ::service_common::validation_message::hashed_enum_order(&declared);
        assert_eq!(
            stated,
            PROCESSOR_TYPE_VALUE_SET.to_vec(),
            "expected {PROCESSOR_TYPE_VALUE_SET:?}, got {stated:?}"
        );
        assert_ne!(
            stated,
            declared.to_vec(),
            "the stated set must not be the model's declaration order, got {stated:?}"
        );
        let mut sorted = declared.to_vec();
        sorted.sort_unstable();
        assert_ne!(
            stated, sorted,
            "the stated set must not be a sort of the values, got {stated:?}"
        );

        // The listing is the model's membership re-ordered, so it cannot drift from the
        // type the model declares.
        let mut sorted_stated = stated.clone();
        sorted_stated.sort_unstable();
        let modelled: Vec<&str> = {
            let mut values = crate::ops::types::shared_types::PROCESSOR_TYPE_VALUES.to_vec();
            values.sort_unstable();
            values
        };
        assert_eq!(
            sorted_stated, modelled,
            "the stated set must hold exactly the modelled values, got {stated:?}"
        );
    }

    /// The order does not depend on the order the values reached the clause in, for a type
    /// whose values fall in distinct buckets: the same six, presented the other way round,
    /// are stated the same way. (A type two of whose values COLLIDE is stated from the
    /// chain each was pushed onto, so for one of those the presented order does decide
    /// which of the two comes first — `service_common::validation_message`'s own table
    /// covers that case.)
    #[test]
    fn the_order_is_presentation_independent_for_a_collision_free_type() {
        let mut reversed = crate::ops::types::shared_types::PROCESSOR_TYPE_VALUES.to_vec();
        reversed.reverse();
        let stated = ::service_common::validation_message::hashed_enum_order(&reversed);
        assert_eq!(
            stated,
            PROCESSOR_TYPE_VALUE_SET.to_vec(),
            "presented as {reversed:?}: expected {PROCESSOR_TYPE_VALUE_SET:?}, got {stated:?}"
        );
    }
}
