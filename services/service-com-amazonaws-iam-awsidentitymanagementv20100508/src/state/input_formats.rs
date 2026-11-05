// SPDX-License-Identifier: CC0-1.0

//! The published form of a value the model-level constraint walk does not reach: a MAP's
//! KEYS.
//!
//! A map key's own `@length` and `@pattern` are not taken by the walk, so the operation that
//! takes such a map is what holds its keys to the published form — and states the refusal in
//! the framework's own words, which is what AWS answers for a map whose keys failed. The
//! ENVELOPE and the per-constraint clauses are the shared rendering
//! ([`::service_common::validation_message::map_key_refusal`]), so a clause about a key
//! cannot drift from the one a reported member gets.
//!
//! The forms the walk DOES report, and the sentences IAM answers them with, live beside this
//! module: [`super::member_forms`] for a value outside a published pattern and
//! [`super::entity_path`] for the one pattern the walk cannot decide.

#[allow(unused_imports)]
use super::*;

/// The message AWS refuses a `PutAccountProperties` whose map carries a KEY outside the
/// published form of `accountPropertyKeyType` with.
///
/// The member is the MAP the key was held in, and the requirements are that key shape's own
/// — its published length ([`super::MIN_ACCOUNT_PROPERTY_KEY_CHARACTERS`],
/// [`super::MAX_ACCOUNT_PROPERTY_KEY_CHARACTERS`]) and its pattern
/// ([`super::ACCOUNT_PROPERTY_KEY_PATTERN`]), read from the one place this crate holds them
/// rather than restated here.
pub fn account_property_key_refusal() -> String {
    ::service_common::validation_message::map_key_refusal(
        ACCOUNT_PROPERTIES_MEMBER,
        MIN_ACCOUNT_PROPERTY_KEY_CHARACTERS as i64,
        MAX_ACCOUNT_PROPERTY_KEY_CHARACTERS as i64,
        ACCOUNT_PROPERTY_KEY_PATTERN,
    )
}

/// The member path a refusal about those keys names: the MAP member that carries them, as
/// the framework spells a member path (lowerCamel).
const ACCOUNT_PROPERTIES_MEMBER: &str = "properties";
