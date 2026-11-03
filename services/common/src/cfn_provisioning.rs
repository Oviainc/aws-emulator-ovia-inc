// SPDX-License-Identifier: CC0-1.0

//! Which Amazon-Web-Services-published CloudFormation RESOURCE types a resource provider
//! stands behind, and which ones none does.
//!
//! A resource type in the CloudFormation registry is provisioned by its resource provider:
//! the handler package whose `create`, `read`, `update` and `delete` handlers CloudFormation
//! calls to make, report and remove a resource of that type. The registry reports which of
//! those a type has as the type version's provisioning behaviour — `FULLY_MUTABLE` for a
//! provider that also updates the resource in place, `NON_PROVISIONABLE` for a type that
//! "doesn't include create, read, and delete handlers, and therefore can't actually be
//! provisioned" — and Cloud Control API, which acts on a resource ONLY through those
//! handlers, has nothing to call for a type in that last class.
//!
//! Two services need the same answer, so it lives here once: CloudFormation REPORTS the
//! class on every registry read of such a type, and Cloud Control API REFUSES an action
//! against a type in it. Held apart, a client could discover a non-provisionable type
//! through the registry and then have Cloud Control API accept a create against it.

/// The Amazon-Web-Services-published RESOURCE types no resource provider stands behind.
///
/// Source: the resource types the CloudFormation User Guide documents as CloudFormation's
/// OWN, provisioned inside the stack that declares them rather than by a provider. A wait
/// condition and its handle are the stack's own synchronisation — the handle IS a pre-signed
/// URL the stack's signals are posted to, and the condition is the stack operation waiting
/// for them — and a custom resource's provisioning is the request CloudFormation sends to
/// the `ServiceToken` its declaration names, which is the author's own function or topic.
/// None of the three publishes a handler package, so none has a create, read or delete
/// handler anything outside a stack operation could call, which is the provisioning class
/// `NON_PROVISIONABLE` states.
///
/// A LIST of names, because that is what this knowledge is: the registry holds a record for
/// each of them, and the class a record is reported in has to be the same one an action
/// against it is judged by.
pub const NON_PROVISIONABLE_RESOURCE_TYPES: &[&str] = &[
    "AWS::CloudFormation::CustomResource",
    "AWS::CloudFormation::WaitCondition",
    "AWS::CloudFormation::WaitConditionHandle",
];

/// Whether the resource type `type_name` is one no resource provider stands behind
/// ([`NON_PROVISIONABLE_RESOURCE_TYPES`]) — so the registry reports it as
/// `NON_PROVISIONABLE` and an API that acts through a provider's handlers has none to call.
///
/// Every other type name, published or not, answers `false`: this module knows which types
/// have NO provider, not which ones exist.
pub fn is_non_provisionable(type_name: &str) -> bool {
    NON_PROVISIONABLE_RESOURCE_TYPES.contains(&type_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogued types are the only ones in the class, and the question is asked of
    /// the WHOLE name: a prefix of one, a suffix of one and a type of another service are
    /// each provisionable as far as this module is concerned.
    #[test]
    fn only_the_catalogued_types_have_no_provider() {
        let cases: &[(&str, bool)] = &[
            ("AWS::CloudFormation::WaitCondition", true),
            ("AWS::CloudFormation::WaitConditionHandle", true),
            ("AWS::CloudFormation::CustomResource", true),
            // A CloudFormation type of its own registry, which a provider does provision.
            ("AWS::CloudFormation::Stack", false),
            // Another service's type, from another namespace entirely.
            ("AWS::SQS::Queue", false),
            // A name that merely CONTAINS a catalogued one, either way round.
            ("AWS::CloudFormation::WaitConditionHandleAlias", false),
            ("My::AWS::CloudFormation::WaitCondition", false),
            // A private extension of a third party, and a value that is no type name.
            ("My::Sample::Resource", false),
            ("", false),
        ];
        for (type_name, expected) in cases {
            let actual = is_non_provisionable(type_name);
            assert_eq!(
                actual, *expected,
                "case {type_name:?}: expected is_non_provisionable {expected}, got {actual}"
            );
        }
    }

    /// The catalogue carries each name once and every entry is a three-segment type name,
    /// so neither a duplicate nor a malformed entry can reach a registry record.
    #[test]
    fn the_catalogue_is_a_set_of_type_names() {
        let mut seen: Vec<&str> = Vec::new();
        for type_name in NON_PROVISIONABLE_RESOURCE_TYPES {
            assert!(
                !seen.contains(type_name),
                "case {type_name:?}: the catalogue carries it twice"
            );
            seen.push(type_name);
            let segments: Vec<&str> = type_name.split("::").collect();
            assert!(
                segments.len() == 3 && segments.iter().all(|segment| !segment.is_empty()),
                "case {type_name:?}: expected three non-empty `::`-separated segments, got \
                 {segments:?}"
            );
        }
    }
}
