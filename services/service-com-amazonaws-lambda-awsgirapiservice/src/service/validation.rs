// SPDX-License-Identifier: CC0-1.0

//! WHERE this service states a constraint one of a COLLECTION's ELEMENTS broke, and WHAT
//! that clause says an element has to satisfy.
//!
//! # The rule
//!
//! The framework's constraint walk reports such a violation at the offending ELEMENT —
//! `<collection>.<1-based position>.member`, naming only the constraint that element broke
//! — while AWS reports it ONCE at the COLLECTION MEMBER, quoting the value that member held
//! (the whole list the caller sent) and nesting, inside
//! `Member must satisfy constraint: [ … ]`, every requirement an ELEMENT of that collection
//! has: the constraints the element shape declares, and — because no list this model
//! declares is `@sparse` — that an element must be present at all.
//!
//! So one request sending a list whose elements are not all admissible is ONE validation
//! error about one member, whatever an element got wrong and however many elements got it
//! wrong.
//!
//! # What is this service's and what is not
//!
//! Only the nested REQUIREMENT is stated here. The envelope, the member path, the value half
//! of the clause, the grouping itself and every clause about a violation that is not an
//! element's are `service-common`'s and the framework's
//! (`service_common::validation_message::aws_validation_refusal_over_elements_stating`,
//! which [`crate::service`] hands this to), so a caller cannot be told the same broken
//! constraint two ways and no sentence is spelled twice in the tree.
//!
//! The requirement is stated here rather than read off the schemas by that shared rendering
//! for two reasons, both properties of THIS model:
//!
//! * every list member of this model's input surface sits INSIDE a structure
//!   (`vpcConfig.subnetIds`, `instanceRequirements.architectures`, `imageConfig.command`,
//!   …), and a member schema carries its own traits and not the members of the shape it
//!   targets — so the member is looked up under its own name in the shape that DECLARES it
//!   ([`LIST_MEMBER_SHAPES`]);
//! * the set AWS states for this service carries the element's presence requirement beside
//!   the constraints the element shape declares.
//!
//! The two renderings [`crate::service`] owns reach the nested set as well, so a requirement
//! inside the brackets reads exactly like one a reported member gets: a `@pattern` without
//! the anchors a whole-value pattern wraps it in, and an enum's value set in the order this
//! service reports that enum in.

use ::aws_smithy_schema::Schema;
use ::service_common::validation::unanchored;
use ::service_common::validation_message::{
    element_constraint_set, element_set_requirement, requirement,
};
use smithy_server_runtime::ViolationKind;

/// The shapes a refused COLLECTION member is looked up in, to state the whole set of
/// requirements that member's ELEMENTS have.
///
/// Every structure of this model's INPUT surface that declares a list member, so that every
/// list a request can carry is reached: the operation inputs that declare one at their top
/// level, and the structures those inputs nest. A member schema carries its own traits and
/// not the members of the shape it targets, which is why the nested structures are handed
/// over themselves rather than reached through the input that carries them.
///
/// A member two of these shapes declare with DIFFERENT element constraints is not stated at
/// all (`service_common::validation_message::element_constraint_set` answers `None` for one,
/// since a refusal does not say which operation it came from): the collection members these
/// shapes share a name for — `subnetIds` and `securityGroupIds`, declared by the VPC
/// configuration of a function and by that of a capacity provider, and `architectures`,
/// declared by two function operations and by the instance requirements — target lists whose
/// ELEMENT shapes are the same (`SubnetId`, `SecurityGroupId`, `Architecture`), so each of
/// them has one set. Source: `smithy_query(query="shape", shape="CapacityProviderSubnetIds,\
/// SubnetIds,CapacityProviderSecurityGroupIds,SecurityGroupIds,ArchitecturesList")`.
///
/// A list of STRUCTURES (`filterCriteria.filters`) and a list whose element shape declares
/// no constraint at all declare no element constraint set, so a violation inside one keeps
/// the member path the walk reported it at.
const LIST_MEMBER_SHAPES: &[&Schema] = {
    use crate::ops::types;

    &[
        // The operation inputs that declare a list member themselves.
        types::CREATE_FUNCTION_INPUT_SCHEMA,
        types::UPDATE_FUNCTION_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_FUNCTION_CODE_INPUT_SCHEMA,
        types::CREATE_EVENT_SOURCE_MAPPING_INPUT_SCHEMA,
        types::UPDATE_EVENT_SOURCE_MAPPING_INPUT_SCHEMA,
        types::PUBLISH_LAYER_VERSION_INPUT_SCHEMA,
        types::UNTAG_RESOURCE_INPUT_SCHEMA,
        types::LIST_DURABLE_EXECUTIONS_BY_FUNCTION_INPUT_SCHEMA,
        // And the structures those inputs nest.
        types::CAPACITY_PROVIDER_VPC_CONFIG_SCHEMA,
        types::INSTANCE_REQUIREMENTS_SCHEMA,
        types::VPC_CONFIG_SCHEMA,
        types::IMAGE_CONFIG_SCHEMA,
        types::CORS_SCHEMA,
        types::ALLOWED_PUBLISHERS_SCHEMA,
        types::EVENT_SOURCE_MAPPING_METRICS_CONFIG_SCHEMA,
    ]
};

/// The requirement a clause states at the collection member `collection` for a constraint
/// one of its ELEMENTS broke — the element shape's whole declared set plus the element's own
/// presence requirement, nested — or `None` when no shape of this model declares a single
/// element constraint set for that member, which leaves the framework's own per-element
/// clause standing.
///
/// `collection` is the member path the clause is stated at, as a violation path spells it;
/// the member is declared in the shape it is nested in, so it is looked up under the last
/// segment of that path ([`LIST_MEMBER_SHAPES`]).
pub(crate) fn element_set(collection: &str) -> Option<String> {
    let member = collection
        .rsplit_once('.')
        .map_or(collection, |(_, member)| member);
    let declared = element_constraint_set(LIST_MEMBER_SHAPES, member)?;
    let mut requirements: Vec<String> = declared.into_iter().map(stated_requirement).collect();
    // No list this model declares is `@sparse`, so an element has to be there at all, and
    // the set states that beside the constraints its shape declares. The sentence is the
    // framework's own for an absent member, so the set spells no requirement of its own.
    requirements.push(requirement(ViolationKind::MissingRequiredMember));
    Some(element_set_requirement(&requirements))
}

/// One requirement of an element shape, as this service states it: the sentence is the
/// framework's (`service_common::validation_message::requirement`) and the two renderings
/// [`crate::service`] owns are applied to it first, so a requirement nested in a
/// collection's clause reads exactly like the same requirement on a reported member.
fn stated_requirement(declared: ViolationKind) -> String {
    requirement(match declared {
        // The CHECK always ran the model's pattern as written; only the rendering of the
        // expression drops the whole-value anchors.
        ViolationKind::Pattern(pattern) => ViolationKind::Pattern(unanchored(&pattern)),
        ViolationKind::EnumValue(allowed) => {
            ViolationKind::EnumValue(super::reported_enum_order(&allowed))
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    //! The set this module states, over the collection members of this model's input
    //! surface rather than the one a report cited. The clause around it, and the status and
    //! error code it travels in, are pinned on the wire in
    //! `tests/all/collection_element_refusals.rs`.

    use super::*;

    /// RULE: a collection member's nested set is the element shape's whole declared set,
    /// each requirement in the framework's words, followed by the element's own presence
    /// requirement — and a member no shape declares a set for is not stated at all.
    ///
    /// The rows span what the set generalises over: the constraint CLASSES an element shape
    /// declares (an enum value set, a length pair with a pattern), how deeply the member is
    /// nested, and a member declared by two shapes over the same element shape.
    #[test]
    fn a_collections_nested_set_is_every_requirement_its_elements_have() {
        let not_null = requirement(ViolationKind::MissingRequiredMember);
        let rows: &[(&str, Option<String>)] = &[
            (
                // The enum element shape `Architecture`, nested one structure deep.
                "instanceRequirements.architectures",
                Some(format!(
                    "Member must satisfy constraint: [Member must satisfy enum value set: \
                     [x86_64, arm64], {not_null}]"
                )),
            ),
            (
                // The same member declared at an operation input's top level: one set.
                "architectures",
                Some(format!(
                    "Member must satisfy constraint: [Member must satisfy enum value set: \
                     [x86_64, arm64], {not_null}]"
                )),
            ),
            (
                // A length pair and a pattern, stated maximum first and with the anchors
                // the model writes dropped.
                "vpcConfig.subnetIds",
                Some(format!(
                    "Member must satisfy constraint: [Member must have length less than or \
                     equal to 1024, Member must have length greater than or equal to 0, \
                     Member must satisfy regular expression pattern: subnet-[0-9a-z]*, \
                     {not_null}]"
                )),
            ),
            (
                "vpcConfig.securityGroupIds",
                Some(format!(
                    "Member must satisfy constraint: [Member must have length less than or \
                     equal to 1024, Member must have length greater than or equal to 0, \
                     Member must satisfy regular expression pattern: sg-[0-9a-zA-Z]*, \
                     {not_null}]"
                )),
            ),
            (
                "instanceRequirements.allowedInstanceTypes",
                Some(format!(
                    "Member must satisfy constraint: [Member must have length less than or \
                     equal to 30, Member must have length greater than or equal to 1, \
                     Member must satisfy regular expression pattern: [a-zA-Z0-9\\.\\*\\-]+, \
                     {not_null}]"
                )),
            ),
            (
                "tagKeys",
                Some(format!(
                    "Member must satisfy constraint: [Member must have length less than or \
                     equal to 128, Member must have length greater than or equal to 1, \
                     Member must satisfy regular expression pattern: \
                     ([\\p{{L}}\\p{{Z}}\\p{{N}}_.:/=+\\-@]*), {not_null}]"
                )),
            ),
            // Members no shape of this model declares an element constraint set for.
            ("filterCriteria.filters", None),
            ("functionName", None),
            ("", None),
        ];

        for (collection, expected) in rows {
            let stated = element_set(collection);
            assert_eq!(
                stated.as_deref(),
                expected.as_deref(),
                "case {collection:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
