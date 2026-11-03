// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request the boundary validator refused before any handler ran.
//!
//! The framework checks a request against the constraints the model declares and hands the
//! service the refusal STRUCTURALLY — one violation per offending position, carrying the
//! member path it sits at, the constraint class it broke and the value it held — and leaves
//! the words to the service, because no model records them. [`refused`] is those words,
//! reached from [`crate::service`]'s `override_refusal`, the one seam that sees a refusal
//! taken before dispatch.
//!
//! Three rules distinguish this service's wording from the framework's default. None is keyed
//! to an operation, a member name or a value, so each holds for every constrained member of
//! every operation of this service:
//!
//! 1. a clause names the offending MEMBER PATH and the CONSTRAINT it broke, and NOTHING of
//!    the value the caller sent — not the submitted bytes, and not the `null` the framework
//!    writes where a member was absent. The requirement states what the request must
//!    satisfy, which the one value that missed it does not help with, and an error body that
//!    quotes request content back turns a request into its own echo;
//! 2. a constraint the model declares on the ELEMENTS of a collection member is stated at
//!    that MEMBER — no element position and no `member` step — with every requirement the
//!    element shape declares nested inside `Member must satisfy constraint: [ … ]`. The whole
//!    set, not just the constraint that happened to fail: a caller is told what each element
//!    of that member has to satisfy. A constraint the COLLECTION itself broke (its own
//!    `@length`, its `@uniqueItems`), and one on a member inside a structure element, keep
//!    their own path and their own requirement;
//! 3. a requirement that enumerates an enumerated type's value set states the set the
//!    SERVICE accepts, which is wider than the public model's for the rule-group type
//!    ([`SERVICE_VALUE_SETS`]).
//!
//! Rules 1 and 2 are AWS's own wording rather than this service's, and both are implemented
//! once for the fleet in `service_common::validation_message`, which this module DELEGATES to
//! rather than rewording: [`service_common::validation_message::aws_validation_refusal_over_elements_withholding`]
//! is the entry point that takes both halves (withhold every value, group over a collection's
//! elements), and the envelope, the member paths, the element sets and every requirement
//! sentence stay the framework's and that module's. Rule 3 is the only wording this service
//! owns, because the value set it states is a fact about THIS service.
//!
//! One limit of that shared rendering, which it states too: an element set is read off the
//! operation input schemas codegen derives from the model, and a member schema carries its own
//! traits and not the members of the shape it targets — so only a TOP-LEVEL collection member
//! resolves, and a collection nested inside a structure keeps the element path and the one
//! requirement its element broke.

use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::smithy_server_runtime::{Refusal, Refused};

/// Enumerated value sets this service's validator enumerates in FULL, each keyed by the set
/// the public model declares for the same type.
///
/// The rule-group type is one enumeration service-side and a subset of it in the public
/// model: `smithy_query(query="shape",shape="RuleGroupType")` declares the three values a
/// caller may name on a create, while a refusal naming the type enumerates every value the
/// service's own enumeration carries, in its order. So the model is not the source for this
/// one requirement's text, and the set below is what the service states.
///
/// Keyed by the WHOLE modelled set, in the model's order, so only the type whose set is
/// restated here is affected and every other enumerated member of this service keeps the
/// values its model declares.
const SERVICE_VALUE_SETS: &[(&[&str], &[&str])] = &[(
    &["STATELESS", "STATEFUL", "STATEFUL_DOMAIN"],
    &[
        "FIREWALL",
        "STATELESS",
        "IPS",
        "STATEFUL",
        "IPS_DOMAIN",
        "STATEFUL_DOMAIN",
    ],
)];

/// Every operation INPUT shape of this service, as the schemas codegen derives from the
/// Smithy model.
///
/// What they are read for: rule 2 states the whole constraint set an element shape declares,
/// and that set is a property of the model rather than of one request — a refusal carries
/// only the constraints a value actually broke. Codegen attaches a collection member's
/// element constraints to that member's schema, which is where the shared rendering reads
/// them, and where the walk read them to refuse the request.
///
/// ALL of them, not the ones one refusal happens to name: the rule holds for every
/// constrained collection of every operation. `grep -h 'super::types::.*_INPUT_SCHEMA'
/// src/ops/*.rs` names exactly these, one per operation file; one missing from this list is
/// not a wrong clause, only a clause stating the constraint that failed rather than the
/// element shape's whole set.
const INPUT_SCHEMAS: &[&Schema] = {
    use crate::ops::types;

    &[
        types::ACCEPT_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::ASSOCIATE_AVAILABILITY_ZONES_INPUT_SCHEMA,
        types::ASSOCIATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::ASSOCIATE_SUBNETS_INPUT_SCHEMA,
        types::ATTACH_RULE_GROUPS_TO_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::CREATE_FIREWALL_INPUT_SCHEMA,
        types::CREATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::CREATE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_PROXY_INPUT_SCHEMA,
        types::CREATE_PROXY_RULES_INPUT_SCHEMA,
        types::CREATE_PROXY_RULE_GROUP_INPUT_SCHEMA,
        types::CREATE_RULE_GROUP_INPUT_SCHEMA,
        types::CREATE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::CREATE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DELETE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::DELETE_FIREWALL_INPUT_SCHEMA,
        types::DELETE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::DELETE_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::DELETE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::DELETE_PROXY_INPUT_SCHEMA,
        types::DELETE_PROXY_RULES_INPUT_SCHEMA,
        types::DELETE_PROXY_RULE_GROUP_INPUT_SCHEMA,
        types::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        types::DELETE_RULE_GROUP_INPUT_SCHEMA,
        types::DELETE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::DELETE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DESCRIBE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_METADATA_INPUT_SCHEMA,
        types::DESCRIBE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::DESCRIBE_FLOW_OPERATION_INPUT_SCHEMA,
        types::DESCRIBE_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_RULE_GROUP_INPUT_SCHEMA,
        types::DESCRIBE_PROXY_RULE_INPUT_SCHEMA,
        types::DESCRIBE_RESOURCE_POLICY_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_METADATA_INPUT_SCHEMA,
        types::DESCRIBE_RULE_GROUP_SUMMARY_INPUT_SCHEMA,
        types::DESCRIBE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
        types::DESCRIBE_VPC_ENDPOINT_ASSOCIATION_INPUT_SCHEMA,
        types::DETACH_RULE_GROUPS_FROM_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::DISASSOCIATE_AVAILABILITY_ZONES_INPUT_SCHEMA,
        types::DISASSOCIATE_SUBNETS_INPUT_SCHEMA,
        types::GET_ANALYSIS_REPORT_RESULTS_INPUT_SCHEMA,
        types::LIST_ANALYSIS_REPORTS_INPUT_SCHEMA,
        types::LIST_CONTAINER_ASSOCIATIONS_INPUT_SCHEMA,
        types::LIST_FIREWALLS_INPUT_SCHEMA,
        types::LIST_FIREWALL_POLICIES_INPUT_SCHEMA,
        types::LIST_FLOW_OPERATIONS_INPUT_SCHEMA,
        types::LIST_FLOW_OPERATION_RESULTS_INPUT_SCHEMA,
        types::LIST_PROXIES_INPUT_SCHEMA,
        types::LIST_PROXY_CONFIGURATIONS_INPUT_SCHEMA,
        types::LIST_PROXY_RULE_GROUPS_INPUT_SCHEMA,
        types::LIST_RULE_GROUPS_INPUT_SCHEMA,
        types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        types::LIST_TLS_INSPECTION_CONFIGURATIONS_INPUT_SCHEMA,
        types::LIST_VPC_ENDPOINT_ASSOCIATIONS_INPUT_SCHEMA,
        types::PUT_RESOURCE_POLICY_INPUT_SCHEMA,
        types::REJECT_NETWORK_FIREWALL_TRANSIT_GATEWAY_ATTACHMENT_INPUT_SCHEMA,
        types::START_ANALYSIS_REPORT_INPUT_SCHEMA,
        types::START_FLOW_CAPTURE_INPUT_SCHEMA,
        types::START_FLOW_FLUSH_INPUT_SCHEMA,
        types::TAG_RESOURCE_INPUT_SCHEMA,
        types::UNTAG_RESOURCE_INPUT_SCHEMA,
        types::UPDATE_AVAILABILITY_ZONE_CHANGE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_CONTAINER_ASSOCIATION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_ANALYSIS_SETTINGS_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_DELETE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_DESCRIPTION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_ENCRYPTION_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_POLICY_CHANGE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_FIREWALL_POLICY_INPUT_SCHEMA,
        types::UPDATE_LOGGING_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_PROXY_CONFIGURATION_INPUT_SCHEMA,
        types::UPDATE_PROXY_INPUT_SCHEMA,
        types::UPDATE_PROXY_RULE_GROUP_PRIORITIES_INPUT_SCHEMA,
        types::UPDATE_PROXY_RULE_INPUT_SCHEMA,
        types::UPDATE_PROXY_RULE_PRIORITIES_INPUT_SCHEMA,
        types::UPDATE_PROXY_SETTINGS_INPUT_SCHEMA,
        types::UPDATE_RULE_GROUP_INPUT_SCHEMA,
        types::UPDATE_SUBNET_CHANGE_PROTECTION_INPUT_SCHEMA,
        types::UPDATE_TLS_INSPECTION_CONFIGURATION_INPUT_SCHEMA,
    ]
};

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a different
/// failure with its own code and status, and nothing about it is this service's to word.
///
/// The error code travels with the message from the shared rendering, which names the one
/// every operation of this service declares for a refused input.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // Rule 3 is applied to the violations BEFORE the clauses are rendered, so the clause
    // itself, its member path and the words around the value set stay the framework's.
    let stated: Vec<Violation> = violations.iter().map(in_service_value_set).collect();
    service_common::validation_message::aws_validation_refusal_over_elements_withholding(
        &Refusal::Invalid(stated),
        INPUT_SCHEMAS,
        // This service quotes a `@pattern` requirement as the MODEL declares it, anchors
        // and all — the framework's own rendering of the expression.
        str::to_string,
        // Rule 1: no clause of this service's refusals states a value, whichever member it
        // is about, so the decision is the same for every member path.
        |_| true,
    )
}

/// `violation` with an enumerated type's value set stated as the SERVICE enumerates it, and
/// any other violation returned unchanged.
///
/// Only the value set is replaced: the member path, the constraint class and the clause's
/// wording keep coming from the one place that writes them.
fn in_service_value_set(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    Violation {
        kind: ViolationKind::EnumValue(stated_value_set(declared)),
        ..violation.clone()
    }
}

/// The value set a requirement states for a member whose model declares `declared`: the
/// service's own set where [`SERVICE_VALUE_SETS`] restates it, and the modelled set
/// otherwise.
fn stated_value_set(declared: &[String]) -> Vec<String> {
    for (modelled, stated) in SERVICE_VALUE_SETS {
        if declared.len() == modelled.len()
            && declared
                .iter()
                .zip(modelled.iter())
                .all(|(declared, modelled)| declared == modelled)
        {
            return stated.iter().map(|value| (*value).to_owned()).collect();
        }
    }
    declared.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an enumerated value set is stated as the service enumerates it where it
    /// restates one, and as the model declares it everywhere else.
    ///
    /// The rows span: the restated set, a set of the same values in another order, a subset
    /// of it, a superset, an unrelated set of this model's and the empty set.
    #[test]
    fn a_value_set_is_restated_only_where_the_service_widens_it() {
        let widened = [
            "FIREWALL",
            "STATELESS",
            "IPS",
            "STATEFUL",
            "IPS_DOMAIN",
            "STATEFUL_DOMAIN",
        ];
        // (case, the set the model declares, the set a requirement states)
        let rows: [(&str, &[&str], &[&str]); 6] = [
            (
                "the rule-group type's modelled set",
                &["STATELESS", "STATEFUL", "STATEFUL_DOMAIN"],
                &widened,
            ),
            (
                "the same values in another order",
                &["STATEFUL", "STATELESS", "STATEFUL_DOMAIN"],
                &["STATEFUL", "STATELESS", "STATEFUL_DOMAIN"],
            ),
            (
                "a subset of the restated set",
                &["STATELESS", "STATEFUL"],
                &["STATELESS", "STATEFUL"],
            ),
            ("the restated set itself", &widened, &widened),
            (
                "the stateful rule order, another enumeration of this model",
                &["DEFAULT_ACTION_ORDER", "STRICT_ORDER"],
                &["DEFAULT_ACTION_ORDER", "STRICT_ORDER"],
            ),
            ("no declared value at all", &[], &[]),
        ];
        for (case, declared, expected) in rows {
            let declared: Vec<String> = declared.iter().map(|value| (*value).to_owned()).collect();
            let stated = stated_value_set(&declared);
            assert_eq!(
                stated, expected,
                "case {case}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: only an enumerated-value violation is touched; every other class reaches the
    /// shared rendering exactly as the walk reported it.
    #[test]
    fn no_other_violation_class_is_touched() {
        for kind in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxValue("100".to_owned()),
            ViolationKind::Pattern("^vpc-[0-9a-f]+$".to_owned()),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "vpcIds".to_owned(),
                kind: kind.clone(),
                value: Some("DMZVPC".to_owned()),
                sensitive: false,
            };
            let stated = in_service_value_set(&violation);
            assert_eq!(
                stated, violation,
                "case {kind:?}: expected the violation unchanged, got {stated:?}"
            );
        }
    }
}
