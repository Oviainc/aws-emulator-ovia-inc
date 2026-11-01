// SPDX-License-Identifier: CC0-1.0

//! Which members of a request this service judges by the value set of the enum behind
//! them, and what it refuses a value outside that set with.
//!
//! RULE: such a member either names something this API defines or names nothing at
//! all, and a value outside the declared set is the second — no cluster, add-on or
//! capability could make the request answerable — so the operation it was sent to
//! answers it as the `InvalidParameterException` (400) it models, in the sentence that
//! operation states for that member, before the resources the request names are
//! resolved.
//!
//! WHERE it is answered is not this module's question, and cannot be a handler: an
//! `enum` shape is generated as a plain `String` field and carries its value set into
//! the operation's input schema as a constraint
//! (`vendor/smithy-macros/src/codegen.rs`), so the framework's validating walk refuses
//! the value BEFORE any handler runs and words it as one more constrained leaf —
//! `Value '<value>' at '<member>' failed to satisfy constraint: …`. The service's own
//! wording therefore has to be decided in the one seam a service has before dispatch
//! (`smithy_server_runtime::Service::override_operation`), by the layer that owns the
//! order the called operation answers its questions in:
//!
//! * `CreateCapability`'s `type` by [`super::precedence`], which already answers that
//!   operation's pass-role authorization and the resolution of the cluster its
//!   `@httpLabel` names, and now answers the type between them;
//! * `CreateAddon`/`UpdateAddon`'s `resolveConflicts` by
//!   [`super::resolve_conflicts_contract`], whose sentence names the operation it was
//!   sent to.
//!
//! What lives HERE is the membership test both use and the refusal each undeclared
//! value earns, so the enum a member is judged against is read from the generated
//! constant for that member's own shape (`crate::ops::types`) — never a list written
//! out — and this service cannot disagree with the walk about what the model declares.

use crate::ops::types::{InvalidParameterException, CAPABILITY_TYPE_VALUES};

/// The value the request carries when it is one `declared` does not hold, or `None`
/// when it carries none or carries a declared one.
///
/// `declared` is the generated value set of the member's own enum shape, so this and
/// the framework's own validating walk read the SAME enum the model declares.
pub fn off_enum<'value>(declared: &[&str], value: Option<&'value str>) -> Option<&'value str> {
    value.filter(|value| !declared.contains(value))
}

/// The refusal `CreateCapability` answers a `type` outside `CapabilityType` with, or
/// `None` for a type this API does define.
///
/// WHICH capability to create is a question about the request alone, so it is answered
/// before the cluster the path names is resolved. The sentence is fixed text in its
/// entirety ([`crate::state::INVALID_CAPABILITY_TYPE_MESSAGE`]) and the payload names
/// no resource: nothing was resolved, so there is no capability and no cluster for it
/// to report.
pub fn undefined_capability_type(value: &str) -> Option<InvalidParameterException> {
    off_enum(CAPABILITY_TYPE_VALUES, Some(value)).map(|_| InvalidParameterException {
        message: Some(crate::state::INVALID_CAPABILITY_TYPE_MESSAGE.to_string()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::RESOLVE_CONFLICTS_VALUES;

    /// The enum's own value set decides what counts as a value this API does not
    /// define, for every member the service judges this way, and a declared value is
    /// never one of them.
    ///
    /// Source for the accepted rows: the value sets the model declares for
    /// `CapabilityType` and `ResolveConflicts`, read through the generated
    /// `CAPABILITY_TYPE_VALUES` and `RESOLVE_CONFLICTS_VALUES`.
    #[test]
    fn the_modelled_enum_decides_which_values_are_undefined() {
        for declared in [CAPABILITY_TYPE_VALUES, RESOLVE_CONFLICTS_VALUES] {
            for value in declared {
                assert!(
                    off_enum(declared, Some(value)).is_none(),
                    "case {value:?}: a value the model declares is not an undefined one"
                );
            }
            assert!(
                off_enum(declared, None).is_none(),
                "case absent: a request that names no value names no undefined one"
            );
        }

        // Rows differ along HOW the value misses the enum: unknown, the right word in
        // the wrong case, a declared value with padding, and the empty string.
        for (declared, values) in [
            (CAPABILITY_TYPE_VALUES, ["EKS", "ack", " KRO", ""]),
            (RESOLVE_CONFLICTS_VALUES, ["BANANA", "overwrite", " NONE", ""]),
        ] {
            for value in values {
                assert_eq!(
                    off_enum(declared, Some(value)),
                    Some(value),
                    "case {value:?}: a value outside the enum is an undefined one"
                );
            }
        }
    }

    /// The capability refusal is the fixed sentence and NO identity member, for every
    /// way a `type` can miss the enum — including a value the caller chose the length
    /// of, which the sentence does not grow with because it echoes nothing.
    #[test]
    fn an_undefined_capability_type_is_refused_with_the_fixed_sentence_and_no_member() {
        for value in CAPABILITY_TYPE_VALUES {
            assert!(
                undefined_capability_type(value).is_none(),
                "case {value:?}: a declared capability type is not refused here"
            );
        }

        let long = "x".repeat(5_000);
        for value in ["EKS", "ack", " KRO", "", long.as_str()] {
            let refusal = undefined_capability_type(value)
                .unwrap_or_else(|| panic!("case {value:?}: an undefined type must be refused"));
            assert_eq!(
                refusal.message.as_deref(),
                Some(crate::state::INVALID_CAPABILITY_TYPE_MESSAGE),
                "case {value:?}: expected the fixed sentence, got {:?}",
                refusal.message
            );
            assert_eq!(
                refusal.cluster_name, None,
                "case {value:?}: the refusal must name no cluster, got {:?}",
                refusal.cluster_name
            );
            assert_eq!(
                refusal.addon_name, None,
                "case {value:?}: the refusal must name no add-on, got {:?}",
                refusal.addon_name
            );
            assert_eq!(
                refusal.nodegroup_name, None,
                "case {value:?}: the refusal must name no node group, got {:?}",
                refusal.nodegroup_name
            );
            assert_eq!(
                refusal.fargate_profile_name, None,
                "case {value:?}: the refusal must name no Fargate profile, got {:?}",
                refusal.fargate_profile_name
            );
            assert_eq!(
                refusal.subscription_id, None,
                "case {value:?}: the refusal must name no subscription, got {:?}",
                refusal.subscription_id
            );
        }
    }
}
