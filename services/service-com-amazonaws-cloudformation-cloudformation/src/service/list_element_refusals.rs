// SPDX-License-Identifier: CC0-1.0

//! A constraint a LIST's ELEMENTS break, stated the way this service states it: at the list
//! member, quoting the list the request sent.
//!
//! ## The rule
//!
//! The framework's constraint walk reports such a violation at the ELEMENT it found it on —
//! `'<list>.<1-based position>.member'`, quoting that one element — because that is where the
//! value is. CloudFormation reports it at the list MEMBER: the subject is the value that
//! member holds, which is the WHOLE list as the request sent it (neither sorted nor
//! deduplicated), and the requirement is the element shape's own constraint nested inside
//! `Member must satisfy constraint: [ … ]`. So a client that matches or reports the refusal
//! sees the member it submitted and the value it submitted for it.
//!
//! That is one rule about every list member of every operation, not about one filter, so it is
//! applied here — once, over the violations the framework's own walk found — rather than in
//! any handler: nothing below re-checks anything, and a request the framework would accept is
//! never refused by this.
//!
//! The grouping, the nesting and the envelope are the tree's shared ones
//! ([`::service_common::validation_message::aws_validation_refusal_over_elements_stating`]);
//! what is this service's own is the ORDER its own validator lists an enum's value set in
//! ([`super::enum_value_set_in_the_services_own_order`] states the same order for a clause
//! about a plain member), and the list itself, which no violation carries and only the
//! request can answer ([`::service_common::validation::refusing_violations_with_lists`]).
//!
//! ## Where it is applied
//!
//! In the operation wrapper every request of this service passes through
//! ([`super::query_error_envelope::RequestIdEnvelopeOperation`]), because the collecting half
//! needs the REQUEST: `Service::override_refusal` is handed the violations alone, and the
//! value a clause states at a list member is not among them. The cost is that a request whose
//! operation declares such a list is read twice — once here and once in the operation's own
//! sequence — which `services/common/src/validation.rs` records as the core seam this
//! re-read exists only until.

use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::aws_smithy_runtime_api::http::Request as RtRequest;
use ::service_common::validation::list_of_element;
use ::service_common::validation_message::{
    aws_validation_refusal_over_elements_stating, element_constraint_set, element_set_requirement,
    requirement,
};
use ::smithy_server_runtime::{Operation, Refusal, Refused, ServerHttpProtocol};

use super::VALIDATION_REFUSAL_CODE;

/// How one operation's refusing violations and submitted lists are collected: the framework's
/// own walk over that operation's input schema, which is why a collector is needed per
/// operation rather than one for the service.
type Collect = fn(&dyn ServerHttpProtocol, &RtRequest) -> Option<(Vec<Violation>, Refused)>;

/// The collector for each operation this module states the refusal of, resolved by the
/// operation's own wire name ([`Operation::NAME`], so no name is spelled twice).
macro_rules! collected_per_operation {
    ( $( $module:ident :: $operation:ident ),* $(,)? ) => {
        /// The collector for the operation `key` names, or `None` for one whose input
        /// declares no list of scalar values — directly or inside a structure it nests (an
        /// operation's deployment targets, its operation preferences) — since those are the
        /// members a clause of this class can be about at all. An operation with no such list
        /// has no list element to report a constraint at, so it is not read twice for nothing.
        fn collector(key: &str) -> Option<Collect> {
            $(
                if key == <crate::ops::$module::$operation<()> as Operation>::NAME {
                    return Some(
                        stated_from_request::<crate::ops::$module::$operation<()>>
                    );
                }
            )*
            None
        }
    };
}

// Every operation of this service whose input declares such a list. The operation TYPE is
// named with a placeholder engine, because nothing here constructs one: only the operation's
// name and its input schema are read, and both are associated constants of the type.
collected_per_operation!(
    continue_update_rollback::ContinueUpdateRollback,
    create_change_set::CreateChangeSet,
    create_stack::CreateStack,
    create_stack_instances::CreateStackInstances,
    create_stack_refactor::CreateStackRefactor,
    create_stack_set::CreateStackSet,
    delete_stack::DeleteStack,
    delete_stack_instances::DeleteStackInstances,
    describe_stack_resource_drifts::DescribeStackResourceDrifts,
    detect_stack_drift::DetectStackDrift,
    detect_stack_set_drift::DetectStackSetDrift,
    import_stacks_to_stack_set::ImportStacksToStackSet,
    list_stack_instance_resource_drifts::ListStackInstanceResourceDrifts,
    list_stack_instances::ListStackInstances,
    list_stack_refactors::ListStackRefactors,
    list_stack_set_operation_results::ListStackSetOperationResults,
    list_stacks::ListStacks,
    update_generated_template::UpdateGeneratedTemplate,
    update_stack::UpdateStack,
    update_stack_instances::UpdateStackInstances,
    update_stack_set::UpdateStackSet,
);

/// The refusal this service states for `req` under the operation `O`, or `None` when the
/// request breaks no constraint of a list's ELEMENTS — which leaves the framework's own
/// answer, and this service's other wordings of it, exactly as they are.
fn stated_from_request<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let (violations, lists) =
        ::service_common::validation::refusing_violations_with_lists::<O>(protocol, req);
    // Only this class is reworded. Everything else — an absent required member, a bound on a
    // plain member, a value outside an enum a plain member targets — keeps the answer it has
    // today.
    if !violations
        .iter()
        .any(|violation| list_of_element(&violation.path).is_some())
    {
        return None;
    }
    let stated = aws_validation_refusal_over_elements_stating(
        &Refusal::Invalid(violations.clone()),
        // A `@pattern` is quoted as the model writes it, which is what the framework's own
        // clause does: this module changes which MEMBER a clause is stated at and nothing
        // about how any requirement reads.
        |pattern| pattern.to_string(),
        // The value a clause stated at a list member quotes: the list the request sent, in
        // the order it sent it.
        |collection| lists.get(collection).cloned(),
        // ...and the requirement it nests, with an enum's value set in the order this service
        // lists it.
        |collection| element_set_as_the_service_states_it(O::INPUT_SCHEMA, collection),
    )?;
    Some((
        violations,
        Refused {
            // The code a CloudFormation client branches on for a refused value, restated
            // because a chosen refusal carries both halves.
            code: VALIDATION_REFUSAL_CODE.to_string(),
            message: stated.message,
        },
    ))
}

/// The requirement a clause stated at `collection` nests: the element shape's whole declared
/// constraint set, with an enum's value set as this SERVICE states it
/// ([`super::stated_value_set`]).
///
/// The set itself is the one the shared reading of the schema produces
/// ([`element_constraint_set`]) — the model's, constraint for constraint — and only an enum's
/// own value set is restated as the service states it. `None` where the schema declares no
/// constraint on that member's elements, which leaves the clause the framework's own.
fn element_set_as_the_service_states_it(
    input_schema: &'static Schema,
    collection: &str,
) -> Option<String> {
    let declared = element_constraint_set(&[input_schema], collection)?;
    let requirements: Vec<String> = declared
        .into_iter()
        .map(|kind| {
            let kind = match kind {
                ViolationKind::EnumValue(values) => {
                    match super::stated_value_set(&values) {
                        Some(stated) => ViolationKind::EnumValue(
                            stated.iter().map(|value| (*value).to_string()).collect(),
                        ),
                        None => ViolationKind::EnumValue(values),
                    }
                }
                other => other,
            };
            requirement(kind)
        })
        .collect();
    Some(element_set_requirement(&requirements))
}

/// The refusal this service states for one request, by the operation its routing key names,
/// or `None` for an operation this class cannot arise in and for a request that breaks no
/// such constraint.
pub fn stated(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let key = protocol.routing_key(req).ok()?;
    collector(key)?(protocol, req)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an operation whose input declares a list of scalars is collected, and one that
    /// declares none is not — so no request is read twice for a clause that cannot arise in
    /// it.
    #[test]
    fn the_operations_collected_are_the_ones_with_a_list_input() {
        for key in ["ListStacks", "CreateStack", "UpdateStack", "DeleteStack"] {
            assert!(
                collector(key).is_some(),
                "case {key:?}: expected the operation to be collected"
            );
        }
        for key in ["DescribeStacks", "GetStackPolicy", "ListExports", "NotAnOperation"] {
            assert!(
                collector(key).is_none(),
                "case {key:?}: expected the operation not to be collected"
            );
        }
    }
}
